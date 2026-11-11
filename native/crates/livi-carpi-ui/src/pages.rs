// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The three pages: what each shows, and what pushing and the D-pad do (docs/PAGES.md 1).

use std::path::PathBuf;
use std::time::Instant;

use carpi_proto::{DiagAction, LightPatch, PinChange, Settings, SettingsPatch, Value};

use crate::PageId;
use crate::client::{Client, Snapshot};
use crate::config::{self, Saved};
use crate::defs::{DEFS, hidden};
use crate::diag::{self, NO_POLL};
use crate::light;
use crate::logging::{self, Logging};
use crate::slots::{self, Board, Editor, Picker, SlotAct, polled, reading, tile};
use crate::values::{Store, show};
use crate::view::{Act, CarScreen, Hit, Layout, Row, RowKind, View};

/// The light model's curve, JSON for the plot: no use as a reading.
const CURVE: &str = "light.curve";

/// The native Settings slot that owns the bundled update screens.
pub const UPDATES_PATH: &str = "/settings/system/softwareUpdate";

mod tab;

use tab::PIN_MAX;

/// After pushing something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    Stay,
    /// Another screen, or the same one moved on: focus the new view's start.
    Opened,
    /// Went back: focus this.
    Closed(i32),
    /// Focus this on the new view.
    Focus(i32),
}

pub struct Pages {
    saved: Saved,
    file: Option<PathBuf>,
    /// The Data page's pages.
    board: Board,
    /// The Data page's slot editor, while editing.
    editor: Option<Editor>,
    module: Option<usize>,
    car: CarScreen,
    /// The first bundled screen of the current native Settings route.
    car_root: CarScreen,
    trail: Vec<i32>,
    /// The portal PIN being entered.
    pin: String,
    /// "Remove this page" waits for a second push.
    removing: bool,
    /// Clearing this module's fault codes waits for a confirm.
    confirm: Option<u8>,
    /// The picker of a cluster display reading.
    reading: Picker,
    /// The Logging screens.
    log: Logging,
    /// The on-screen keyboard while it's up, and what it's typing.
    typing: Option<(crate::keyboard::Keyboard, crate::view::TypeFor)>,
    /// "Forget" waits for a second push.
    forgetting: bool,
    /// A touch screen is attached: the keyboard's Auto opens the touch layout.
    touch: bool,
    /// What the keyboard typed into one of LIVI's fields, until it's taken.
    field: Option<String>,
    /// The screen Setup opened: back from it goes to Setup, not its parent.
    via_setup: Option<CarScreen>,
    /// Starting Move to NVMe waits for a second push.
    moving: bool,
    /// A firmware install waits for a second push.
    flashing: Option<crate::firmware::Unit>,
    /// Flash Sector armed (upload name), waiting for the second push.
    flashing_sector: Option<String>,
}

impl Pages {
    /// With the pages from `~/.config/LIVI/carpi-ui.json`.
    pub fn load() -> Self {
        let file = config::path();
        let saved = file.as_deref().map(config::load).unwrap_or_default();
        Self::new(saved, file)
    }

    /// `file` is where changes go; `None` keeps them in memory.
    pub fn new(saved: Saved, file: Option<PathBuf>) -> Self {
        Self {
            board: saved.board(),
            saved,
            file,
            editor: None,
            module: None,
            car: CarScreen::Root,
            car_root: CarScreen::Root,
            trail: Vec::new(),
            pin: String::new(),
            removing: false,
            confirm: None,
            reading: Picker::default(),
            log: Logging::default(),
            typing: None,
            forgetting: false,
            touch: false,
            field: None,
            via_setup: None,
            moving: false,
            flashing: None,
            flashing_sector: None,
        }
    }

    /// Closes the keyboard without saving (its screen went away).
    pub fn close_keyboard(&mut self) {
        self.typing = None;
    }

    /// The on-screen keyboard, while it's up.
    pub fn keyboard(&self) -> Option<&crate::keyboard::Keyboard> {
        self.typing.as_ref().map(|(k, _)| k)
    }

    pub fn keyboard_prefs(&self) -> crate::keyboard::Prefs {
        self.saved.keyboard
    }

    pub(crate) fn set_keyboard_prefs(&mut self, prefs: crate::keyboard::Prefs) {
        self.saved.keyboard = prefs;
        self.store();
    }

    /// A touch screen is (or isn't) attached now.
    pub fn set_touch(&mut self, attached: bool) {
        self.touch = attached;
    }

    /// The keyboard's prefs with Auto settled, and Off as Knob: on our own fields a real
    /// keyboard types into our keyboard's field.
    fn layout(&self) -> crate::keyboard::Prefs {
        let mut prefs = self.saved.keyboard.layout(self.touch);
        if prefs.style == crate::keyboard::Style::Off {
            prefs.style = crate::keyboard::Style::Knob;
        }
        prefs
    }

    /// Opens the keyboard for one of LIVI's own text fields, holding `max` characters at most.
    /// False with the keyboard Off: LIVI's field takes a real keyboard as upstream.
    pub fn type_field(&mut self, title: &str, initial: &str, secret: bool, max: usize) -> bool {
        if self.saved.keyboard.style == crate::keyboard::Style::Off {
            return false;
        }
        let mut k = crate::keyboard::Keyboard::open(title, initial, secret, "Save", self.layout());
        k.max = max;
        self.field = None;
        self.typing = Some((k, crate::view::TypeFor::Field));
        true
    }

    /// The keyboard is typing one of LIVI's fields.
    pub fn typing_field(&self) -> bool {
        matches!(self.typing, Some((_, crate::view::TypeFor::Field)))
    }

    /// What Done typed into LIVI's field, once.
    pub fn field_typed(&mut self) -> Option<String> {
        self.field.take()
    }

    pub(crate) fn forgetting(&self) -> bool {
        self.forgetting
    }

    /// Whether the Setup card in LIVI's main Settings has been hidden.
    pub fn setup_done(&self) -> bool {
        self.saved.setup_done
    }

    pub(crate) fn set_setup_done(&mut self, done: bool) {
        self.saved.setup_done = done;
        self.store();
    }

    pub(crate) fn moving(&self) -> bool {
        self.moving
    }

    pub(crate) fn flashing(&self) -> Option<crate::firmware::Unit> {
        self.flashing
    }

    pub(crate) fn flashing_sector(&self) -> Option<&str> {
        self.flashing_sector.as_deref()
    }

    /// Where back goes from the Car tab's screen now.
    fn car_parent(&self) -> Option<CarScreen> {
        if self.car == self.car_root {
            return None;
        }
        if self.via_setup == Some(self.car) {
            return Some(CarScreen::Setup);
        }
        self.car.parent()
    }

    /// Opens the keyboard for `what`.
    pub(crate) fn type_for(&mut self, what: crate::view::TypeFor, client: &Client) {
        use crate::view::TypeFor as T;
        let s = client.snapshot().settings.clone();
        let (title, initial, secret, action) = match &what {
            T::WifiName => ("Network name".to_string(), String::new(), false, "Next"),
            T::WifiPassword(ssid) => (format!("Password for {ssid}"), String::new(), true, "Join"),
            T::Accel(i) => (
                format!("Accel test line {}", i + 1),
                s.as_ref()
                    .and_then(|s| s.dis.accel.get(*i).cloned())
                    .unwrap_or_default(),
                false,
                "Save",
            ),
            T::PageName(p) => (
                "Readings page name".to_string(),
                s.as_ref()
                    .and_then(|s| {
                        crate::settings::dis_pages(&s.dis)
                            .get(*p)
                            .map(|pg| pg.name.clone())
                    })
                    .unwrap_or_default(),
                false,
                "Save",
            ),
            T::Field => return,
        };
        let mut k =
            crate::keyboard::Keyboard::open(&title, &initial, secret, action, self.layout());
        if matches!(what, T::PageName(_)) {
            k.max = 20;
        }
        if matches!(what, T::WifiName) {
            k.max = 32;
        }
        self.typing = Some((k, what));
    }

    /// A key while the keyboard is up. False when it isn't.
    pub fn keyboard_input(&mut self, input: crate::keyboard::Input, client: &Client) -> bool {
        use crate::keyboard::Outcome;
        let Some((k, _)) = self.typing.as_mut() else {
            return false;
        };
        match k.input(input) {
            Outcome::Open => {}
            Outcome::Cancelled => self.typing = None,
            Outcome::Done(text) => {
                let (_, what) = self.typing.take().expect("open");
                self.typed(what, text, client);
            }
        }
        true
    }

    fn typed(&mut self, what: crate::view::TypeFor, text: String, client: &Client) {
        use crate::view::TypeFor as T;
        use carpi_proto::WifiAction as W;
        if what == T::Field {
            // As typed: LIVI's own field checks it.
            self.field = Some(text);
            return;
        }
        let text = text.trim().to_string();
        match what {
            T::Field => {}
            T::WifiName if text.is_empty() => {}
            T::WifiName => self.type_for(T::WifiPassword(text), client),
            T::WifiPassword(ssid) => {
                client.wifi(W::Add {
                    ssid,
                    password: (!text.is_empty()).then_some(text),
                    hidden: false,
                });
                if self.car == CarScreen::WifiAdd {
                    self.car = CarScreen::Wifi;
                }
            }
            T::Accel(i) => {
                let Some(s) = client.snapshot().settings.clone() else {
                    return;
                };
                let mut lines = s.dis.accel.clone();
                if text.is_empty() {
                    if i < lines.len() {
                        lines.remove(i);
                    }
                } else if i < lines.len() {
                    lines[i] = text;
                } else {
                    lines.push(text);
                }
                client.set(SettingsPatch {
                    dis: Some(carpi_proto::DisPatch {
                        accel: Some(lines),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
            T::PageName(p) => {
                let Some(s) = client.snapshot().settings.clone() else {
                    return;
                };
                let mut pages = crate::settings::dis_pages(&s.dis);
                if let Some(pg) = pages.get_mut(p).filter(|_| !text.is_empty()) {
                    pg.name = text;
                    client.set(SettingsPatch {
                        dis: Some(carpi_proto::DisPatch {
                            pages: Some(pages),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
            }
        }
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn recent(&self) -> &[String] {
        &self.saved.recent
    }

    pub fn depth(&self, page: PageId) -> usize {
        match page {
            PageId::Data => match &self.editor {
                None => 0,
                Some(Editor { picker: None, .. }) => 1,
                Some(Editor {
                    picker: Some(p), ..
                }) => 2 + usize::from(p.cat.is_some()),
            },
            PageId::Diagnostics => usize::from(self.module.is_some()),
            PageId::Car => {
                let picking = matches!(self.car, CarScreen::CellValue(..) | CarScreen::CellAdd(_))
                    && self.reading.cat.is_some();
                let channel = match (&self.log.editor.picker, self.car) {
                    (Some(p), CarScreen::LogChannels) => 1 + usize::from(p.cat.is_some()),
                    _ => 0,
                };
                // Under a screen Setup opened, Setup and the first screen are above it.
                let base = match self.via_setup {
                    Some(v) if self.car == v => 2,
                    Some(v)
                        if std::iter::successors(self.car.parent(), |s| s.parent())
                            .any(|s| s == v) =>
                    {
                        self.car.depth() - v.depth() + 2
                    }
                    _ => self.car.depth(),
                };
                base.saturating_sub(self.car_root.depth()) + usize::from(picking) + channel
            }
        }
    }

    /// Diag catalog names to keep subscribed, so carpid polls them: the shown module's on the
    /// diagnostics page, the shown page's on the Data page.
    pub fn watching(&self, page: PageId, snap: &Snapshot) -> Vec<String> {
        match page {
            PageId::Data => self
                .board
                .page()
                .shown()
                .iter()
                .flatten()
                .filter(|n| polled(n))
                .cloned()
                .collect(),
            PageId::Diagnostics => {
                let Some(i) = self.module else {
                    return Vec::new();
                };
                snap.diag
                    .modules()
                    .get(i)
                    .map(|m| {
                        diag::measured(m.module.tp2)
                            .map(|v| v.0.to_string())
                            .collect()
                    })
                    .unwrap_or_default()
            }
            PageId::Car => Vec::new(),
        }
    }

    /// The UI path while `page` is shown. The light screens count as a page of their own.
    pub fn path(&self, page: PageId) -> &'static str {
        match (page, self.car) {
            (PageId::Car, _) if self.car_root == CarScreen::Updates => UPDATES_PATH,
            (PageId::Car, screen) if screen.light() => crate::LIGHT_PATH,
            _ => page.path(),
        }
    }

    /// The page's first screen, or the screen `rest` names below it (`light` or `logging` on
    /// the Car page).
    pub fn open(&mut self, page: PageId, rest: &str) {
        self.reset(page);
        let screen = match rest.split('/').next() {
            Some("light") => CarScreen::Lighting,
            Some("logging") => CarScreen::Logging,
            _ => return,
        };
        if page == PageId::Car {
            self.car = screen;
            self.trail = vec![0; screen.depth()];
        }
    }

    /// The Car Integration tab's screen now, and straight to one (for paths and pictures).
    pub fn screen(&self) -> CarScreen {
        self.car
    }

    pub fn goto(&mut self, screen: CarScreen) {
        self.reset(PageId::Car);
        self.car = screen;
        self.trail = vec![0; screen.depth()];
    }

    /// Open the main Settings checklist and refresh its network and storage state.
    pub fn open_setup(&mut self, client: &Client) {
        self.goto(CarScreen::Setup);
        client.wifi(carpi_proto::WifiAction::List);
        client.storage(carpi_proto::StorageAction::Status);
    }

    /// Open the bundled updater in LIVI's native Software Update slot. Status refresh is
    /// passive: it preserves an ongoing job and never starts a download.
    pub fn open_updates(&mut self, client: &Client) {
        self.goto(CarScreen::Updates);
        self.car_root = CarScreen::Updates;
        self.trail.clear();
        client.update_status();
    }

    /// Back to the page's first screen.
    pub fn reset(&mut self, page: PageId) {
        match page {
            PageId::Data => self.editor = None,
            PageId::Diagnostics => {
                self.module = None;
                self.confirm = None;
            }
            PageId::Car => {
                self.car = CarScreen::Root;
                self.car_root = CarScreen::Root;
                self.reading = Picker::default();
                self.log.reset();
                self.pin.clear();
                self.removing = false;
                self.via_setup = None;
                self.moving = false;
                self.flashing = None;
                self.flashing_sector = None;
            }
        }
        self.trail.clear();
    }

    /// One screen up. `None` on a page's first screen, where back leaves the page.
    pub fn back(&mut self, page: PageId) -> Option<Moved> {
        match page {
            PageId::Data => {
                let e = self.editor.as_mut()?;
                if !e.back() {
                    self.editor = None;
                }
                Some(Moved::Opened)
            }
            PageId::Diagnostics => {
                self.module.take()?;
                self.confirm = None;
                Some(Moved::Closed(self.trail.pop().unwrap_or(0)))
            }
            PageId::Car => {
                if matches!(self.car, CarScreen::CellValue(..) | CarScreen::CellAdd(_))
                    && self.reading.back()
                {
                    return Some(Moved::Opened);
                }
                if self.car == CarScreen::LogChannels && self.log.editor.back() {
                    return Some(Moved::Opened);
                }
                let up = self.car_parent()?;
                if self.via_setup == Some(self.car) {
                    self.via_setup = None;
                }
                self.car = up;
                self.moving = false;
                self.flashing = None;
                self.flashing_sector = None;
                self.log.removing = false;
                self.removing = false;
                if self.car == CarScreen::Logging {
                    self.log.open = None;
                }
                self.reading = Picker::default();
                if self.car != CarScreen::Pin {
                    self.pin.clear();
                }
                Some(Moved::Closed(self.trail.pop().unwrap_or(0)))
            }
        }
    }

    /// Pushes what `hit` points at on `view`, the view of `page` the user saw. `focus` is where
    /// focus was, to come back to.
    pub fn press(
        &mut self,
        page: PageId,
        view: &View,
        hit: &Hit,
        focus: i32,
        client: &Client,
    ) -> Moved {
        if *hit == Hit::Back {
            return self.back(page).unwrap_or(Moved::Stay);
        }
        match view.act(hit) {
            Some(act) => self.run(page, act.clone(), focus, client),
            None => Moved::Stay,
        }
    }

    /// The D-pad on `hit`: `delta` < 0 is left.
    pub fn side(
        &mut self,
        page: PageId,
        view: &View,
        hit: Option<&Hit>,
        delta: i32,
        focus: i32,
        client: &Client,
    ) -> Moved {
        match view.side_act(hit, delta) {
            Some(act) => self.run(page, act.clone(), focus, client),
            None => Moved::Stay,
        }
    }

    fn run(&mut self, page: PageId, act: Act, focus: i32, client: &Client) -> Moved {
        if !matches!(act, Act::ClearAsk(_)) {
            self.confirm = None;
        }
        if let Some(moved) = self.tab_run(&act, client) {
            return moved;
        }
        let opened = match act {
            Act::None => false,
            Act::Slots(a) => return self.slots(page, &a, client),
            Act::Edit => {
                self.editor = Some(Editor::default());
                return Moved::Opened;
            }
            Act::Module(i) => {
                self.module = Some(i);
                true
            }
            Act::Car(screen) => {
                self.car = screen;
                self.reading = Picker::default();
                if screen == CarScreen::LogChannels {
                    self.log.editor = Editor::default();
                }
                screen != CarScreen::Root
            }
            Act::Log(a) => {
                let st = client.snapshot().datalog.clone();
                match self.log.apply(&a, st.as_ref(), client) {
                    logging::Done::Stay => false,
                    logging::Done::Opened(screen) => {
                        self.car = screen;
                        true
                    }
                    logging::Done::Back => return self.back(page).unwrap_or(Moved::Stay),
                }
            }
            Act::Capture => {
                client.capture();
                false
            }
            Act::Patch(patch, back) => {
                client.set(*patch);
                if back {
                    return self.back(page).unwrap_or(Moved::Stay);
                }
                false
            }
            Act::PinDigit(d) => {
                if self.pin.len() < PIN_MAX {
                    self.pin.push(char::from(b'0' + d % 10));
                }
                false
            }
            Act::PinDelete => {
                self.pin.pop();
                false
            }
            Act::Diag(action) => {
                client.diag(action);
                false
            }
            Act::DecoderCrc => {
                client.decoder_crc();
                false
            }
            Act::ClearAsk(module) => {
                self.confirm = Some(module);
                false
            }
            Act::ClearCancel => false,
            Act::Light(action) => {
                client.light(action);
                false
            }
            Act::Advanced => false,
            Act::Dis(_)
            | Act::Zone(_)
            | Act::Update
            | Act::Back
            | Act::Type(_)
            | Act::Wifi(_)
            | Act::Keyboard(_)
            | Act::SetupDone(_)
            | Act::PhoneSettings
            | Act::MoveNvme
            | Act::FirmwareProbe
            | Act::FirmwareInstall(_)
            | Act::FlashSector(_) => false,
            Act::PinSave => {
                let pin = std::mem::take(&mut self.pin);
                client.set(SettingsPatch {
                    portal_pin: Some(PinChange::Set { pin }),
                    ..Default::default()
                });
                return self.back(page).unwrap_or(Moved::Stay);
            }
        };
        if opened {
            self.trail.push(focus);
            Moved::Opened
        } else {
            Moved::Stay
        }
    }

    /// The Data page's tabs and editor, and the cluster display's reading picker.
    fn slots(&mut self, page: PageId, a: &SlotAct, client: &Client) -> Moved {
        if page == PageId::Car && self.car == CarScreen::LogChannels {
            let before = self.saved.recent.clone();
            let (focus, _) = self.log.slots(a, &mut self.saved.recent, client);
            if self.saved.recent != before {
                self.store();
            }
            return match focus {
                slots::Focus::Start => Moved::Opened,
                slots::Focus::Stay => Moved::Stay,
                slots::Focus::At(i) => Moved::Focus(i),
            };
        }
        if page == PageId::Car {
            return self.cell_pick(a, client);
        }
        let Some(e) = &mut self.editor else {
            // The plain page: tabs and the D-pad.
            let n = self.board.pages.len();
            let moved = match a {
                SlotAct::Page(i) => {
                    self.board.shown = (*i).min(n - 1);
                    Moved::Stay
                }
                SlotAct::PageStep(d) => {
                    self.board.shown =
                        (self.board.shown as i32 + d).clamp(0, n as i32 - 1) as usize;
                    Moved::Focus(self.board.shown as i32)
                }
                _ => return Moved::Stay,
            };
            self.store();
            return moved;
        };
        let (focus, save) = e.apply(a, &mut self.board, &mut self.saved.recent, &slots::DATA);
        if save {
            self.store();
        }
        match focus {
            slots::Focus::Start => Moved::Opened,
            slots::Focus::Stay => Moved::Stay,
            slots::Focus::At(i) => Moved::Focus(i),
        }
    }

    fn store(&mut self) {
        self.saved.data = Some(self.board.clone());
        let Some(file) = &self.file else { return };
        if let Err(e) = config::save(file, &self.saved) {
            eprintln!("[carpi-ui] can't save the pages: {e}");
        }
    }

    pub fn view(&self, page: PageId, snap: &Snapshot, now: Instant) -> View {
        match page {
            PageId::Data => match &self.editor {
                Some(e) => {
                    let mut v = e.view(&self.board, &self.saved.recent, snap, now, &slots::DATA);
                    if !snap.connected {
                        v.note = problem(snap);
                    }
                    v
                }
                None => self.grid(snap, now),
            },
            PageId::Diagnostics => match self.module {
                Some(i) => module(snap, now, i, self.confirm),
                None => modules(snap),
            },
            PageId::Car => tab::view(self, snap, now),
        }
    }

    fn grid(&self, snap: &Snapshot, now: Instant) -> View {
        let page = self.board.page();
        let mut v = View::new(Layout::Grid, "Data", false);
        for (i, p) in self.board.pages.iter().enumerate() {
            v.pill(
                p.name.clone(),
                i == self.board.shown,
                Act::Slots(SlotAct::Page(i)),
            )
            .side = Some(steps());
        }
        v.pill("Edit", false, Act::Edit).side = Some(steps());
        v.side = Some(steps());
        v.cols = slots::cols(page.size);
        for slot in page.shown() {
            v.cell(tile(slot.as_deref(), snap, now), false);
        }
        v.note = if !snap.connected {
            problem(snap)
        } else if page.shown().iter().all(Option::is_none) {
            "Nothing on this page yet. Edit adds values.".into()
        } else {
            String::new()
        };
        v.start = self.board.shown as i32;
        v.finish()
    }
}

/// The D-pad on the Data page: the page before or after.
fn steps() -> (Act, Act) {
    (
        Act::Slots(SlotAct::PageStep(-1)),
        Act::Slots(SlotAct::PageStep(1)),
    )
}

fn problem(snap: &Snapshot) -> String {
    snap.problem
        .clone()
        .unwrap_or_else(|| "carpid not connected".into())
}

/// A value as an info row: its label and reading, dimmed when not fresh. One line why when
/// there is no reading.
fn value_row(name: &str, snap: &Snapshot, now: Instant) -> Row {
    let seen = snap.values.get(name);
    let s = show(name, seen, now);
    let value = match seen {
        None => "not seen since start".to_string(),
        Some(x) if x.value.is_none() => "no value".to_string(),
        Some(_) if s.freshness.dim() => format!("{} · {}", reading(&s.value, &s.unit), s.age),
        Some(_) => reading(&s.value, &s.unit),
    };
    Row::new(RowKind::Info, s.label, value).dimmed(s.freshness.dim())
}

/// Known names under `prefix` in table order, then any others carpid has.
fn names_under(prefix: &str, store: &Store) -> Vec<String> {
    let mut names: Vec<String> = DEFS
        .iter()
        .filter(|d| d.name.starts_with(prefix))
        .map(|d| d.name.to_string())
        .collect();
    for (n, _) in store.under(prefix) {
        if !names.iter().any(|m| m == n) && !hidden(n) && n != CURVE {
            names.push(n.to_string());
        }
    }
    names
}

/// The line under a diagnostics screen's header: no connection, or the job running or last run.
fn diag_note(snap: &Snapshot, module: Option<u8>) -> String {
    if !snap.connected {
        return problem(snap);
    }
    snap.diag.line(module).unwrap_or_default()
}

fn modules(snap: &Snapshot) -> View {
    let diag = &snap.diag;
    let mut v = View::new(Layout::List, "Diagnostics", false).note(diag_note(snap, None));
    if snap.connected && !diag.busy() {
        let label = if diag.installed.is_some() {
            "Scan again"
        } else {
            "Scan"
        };
        v.pill(label, false, Act::Diag(DiagAction::InstallList));
    }
    v.section(if diag.installed.is_some() {
        "Installed modules"
    } else {
        "Modules (not scanned yet)"
    });
    for (i, m) in diag.modules().iter().enumerate() {
        let faults = diag.dtcs.get(&m.module.tp2).map(Vec::len);
        let value = match (faults, m.coded) {
            (Some(0), _) => "OK".to_string(),
            (Some(1), _) => "1 fault".to_string(),
            (Some(n), _) => format!("{n} faults"),
            (None, Some(false)) => "not coded".to_string(),
            (None, _) => String::new(),
        };
        let mut row = Row::new(
            RowKind::Route,
            format!("{:02X}  {}", m.module.address, m.module.name),
            value,
        )
        .act(Act::Module(i));
        row.alert = faults.is_some_and(|n| n > 0);
        v.row(row);
    }
    v.finish()
}

/// A diag job's button: dimmed with "busy" while another runs.
fn job_row(label: &str, word: &str, free: bool, act: Act) -> Row {
    if free {
        Row::new(RowKind::Button, label, "").button(word).act(act)
    } else {
        Row::new(RowKind::Info, label, "busy").dimmed(true)
    }
}

fn module(snap: &Snapshot, now: Instant, i: usize, confirm: Option<u8>) -> View {
    let diag = &snap.diag;
    let list = diag.modules();
    let Some(m) = list.get(i) else {
        return View::new(Layout::List, "Diagnostics", true).finish();
    };
    let (addr, tp2) = (m.module.address, m.module.tp2);
    let note = diag_note(snap, Some(tp2));
    let mut v = View::new(Layout::List, format!("{addr:02X}  {}", m.module.name), true).note(note);
    let free = snap.connected && !diag.busy();

    v.section("Fault codes");
    match diag.dtcs.get(&tp2) {
        Some(codes) if codes.is_empty() => {
            v.row(Row::new(RowKind::Info, "No fault codes", ""));
        }
        Some(codes) => {
            for c in codes {
                let mut row = Row::new(
                    RowKind::Info,
                    c.code.clone(),
                    format!("status {:02X}", c.status),
                );
                row.alert = true;
                row.mono = true;
                v.row(row);
            }
        }
        None => {
            v.row(Row::new(RowKind::Info, "Not read yet", "").dimmed(true));
        }
    }
    v.row(job_row(
        "Read fault codes",
        "Read",
        free,
        Act::Diag(DiagAction::ReadDtcs { module: tp2 }),
    ));
    if confirm == Some(tp2) {
        let mut sure = job_row(
            "Clear all of this module's codes?",
            "Clear",
            free,
            Act::Diag(DiagAction::ClearDtcs {
                module: tp2,
                confirm: true,
            }),
        );
        sure.alert = true;
        v.row(sure);
        v.row(
            Row::new(RowKind::Button, "Keep them", "")
                .button("Cancel")
                .act(Act::ClearCancel),
        );
    } else {
        v.row(job_row(
            "Clear fault codes",
            "Clear",
            free,
            Act::ClearAsk(tp2),
        ));
    }

    v.section("Measuring values");
    let mut any = false;
    for &(name, _, group, field) in diag::measured(tp2) {
        any = true;
        let mut row = value_row(name, snap, now);
        if snap.values.get(name).is_none() {
            row.value = if snap.connected {
                "polling…".into()
            } else {
                String::new()
            };
        }
        row.tag = format!("{group:03}.{field}");
        v.row(row);
    }
    if !any {
        let why = if tp2 == NO_POLL {
            "Never polled: it upsets ESP and the Haldex"
        } else {
            "None in carpid's catalog"
        };
        v.row(Row::new(RowKind::Info, why, "").dimmed(true));
    }

    v.section("Module");
    let mut id = Row::new(
        RowKind::Info,
        "Address",
        format!("{addr:02X} (TP2 {tp2:02X})"),
    );
    id.mono = true;
    v.row(id);
    if let Some(coded) = m.coded {
        v.row(Row::new(
            RowKind::Info,
            "Coded",
            if coded { "yes" } else { "no" },
        ));
    }
    if let Some(s) = m.status {
        let mut status = Row::new(RowKind::Info, "Install list status", format!("{s:02X}"));
        status.mono = true;
        v.row(status);
    }
    v.finish()
}

fn num(snap: &Snapshot, name: &str) -> Option<f64> {
    match snap.values.get(name)?.value.as_ref()? {
        Value::Num(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

fn text_of<'a>(snap: &'a Snapshot, name: &str) -> Option<&'a str> {
    match snap.values.get(name)?.value.as_ref()? {
        Value::Text(t) => Some(t),
        _ => None,
    }
}

fn flag(snap: &Snapshot, name: &str) -> Option<bool> {
    match snap.values.get(name)?.value.as_ref()? {
        Value::Bool(b) => Some(*b),
        Value::Num(n) => Some(*n != 0.0),
        Value::Text(_) => None,
    }
}

/// The line under a settings screen's header: no connection, a failed change, or a restart to
/// do.
fn settings_note(snap: &Snapshot) -> String {
    if !snap.connected {
        return problem(snap);
    }
    let mut lines = Vec::new();
    if let Some(n) = &snap.settings_note {
        lines.push(n.clone());
    }
    if !snap.restart_needed.is_empty() {
        lines.push(format!(
            "Saved. Takes effect after carpid restarts: {}.",
            snap.restart_needed.join(", ")
        ));
    }
    lines.join(" ")
}

fn patch(p: SettingsPatch, back: bool) -> Act {
    Act::Patch(Box::new(p), back)
}

fn toggle(label: &str, on: bool, act: Act) -> Row {
    Row::new(RowKind::Toggle, label, "").act(act).checked(on)
}

/// A settings screen, or one line saying they haven't come yet.
fn with_settings(snap: &Snapshot, title: &str, body: impl FnOnce(&mut View, &Settings)) -> View {
    let mut v = View::new(Layout::List, title, true).note(settings_note(snap));
    match &snap.settings {
        Some(s) => body(&mut v, s),
        None => {
            v.row(Row::new(RowKind::Info, "Waiting for carpid's settings", "").dimmed(true));
        }
    }
    v.finish()
}

fn light_patch(p: LightPatch, back: bool) -> Act {
    patch(
        SettingsPatch {
            light: Some(p),
            ..Default::default()
        },
        back,
    )
}

/// An ambient estimate with the sensor reading it stands for.
fn ambient_text(a: f64) -> String {
    format!("{a:.2} (sensor {:.0})", light::raw(a))
}

fn capital(s: &str) -> String {
    match s.get(..1) {
        Some(f) => f.to_uppercase() + &s[1..],
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use carpi_proto::Reading;
    use std::time::Duration;

    use super::*;
    use crate::client::Capture;

    #[test]
    fn sdlink_rows_hold_the_reading_between_normal_status_samples() {
        let now = Instant::now();
        let mut s = Snapshot::default();
        for age_ms in [0, 1000, 2000, 3100, 4000, 5000] {
            s.values.take(
                vec![Reading {
                    name: "avsd.fps".into(),
                    value: Some(Value::Num(59.6)),
                    age_ms,
                }],
                now,
            );
            let row = value_row("avsd.fps", &s, now + Duration::from_millis(500));
            assert_eq!(row.value, "59.6 fps", "source age {age_ms} ms");
            assert!(!row.dimmed);
        }
        let row = value_row("avsd.fps", &s, now + Duration::from_millis(3001));
        assert!(row.dimmed);
        assert!(
            row.value.contains(" · "),
            "a disconnected link still shows the age"
        );
    }

    fn snap(values: &[(&str, Option<Value>)]) -> Snapshot {
        let mut s = Snapshot {
            connected: true,
            problem: None,
            ..Snapshot::default()
        };
        s.values.take(
            values
                .iter()
                .map(|(n, v)| Reading {
                    name: n.to_string(),
                    value: v.clone(),
                    age_ms: 0,
                })
                .collect(),
            Instant::now(),
        );
        s
    }

    fn find(v: &View, label: &str) -> usize {
        v.rows
            .iter()
            .position(|r| r.label == label)
            .unwrap_or_else(|| panic!("no row {label}"))
    }

    #[test]
    fn updates_route_keeps_wifi_children_and_confirm_back_inside_its_native_slot() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[]);
        s.settings = Some(crate::settings::sample());
        s.update_recovering = false;
        s.wifi.networks = Some(vec![carpi_proto::WifiNetwork {
            ssid: "Garage".into(),
            secured: true,
            hidden: false,
            last_used: None,
        }]);
        let now = Instant::now();
        p.open_updates(&client);
        assert_eq!(
            (p.depth(PageId::Car), p.path(PageId::Car)),
            (0, UPDATES_PATH)
        );
        let v = p.view(PageId::Car, &s, now);
        p.press(
            PageId::Car,
            &v,
            &Hit::Row(find(&v, "Wi-Fi networks")),
            5,
            &client,
        );
        assert_eq!((p.screen(), p.depth(PageId::Car)), (CarScreen::Wifi, 1));
        let v = p.view(PageId::Car, &s, now);
        p.press(PageId::Car, &v, &Hit::Row(find(&v, "Garage")), 2, &client);
        assert_eq!(
            (p.screen(), p.depth(PageId::Car)),
            (CarScreen::WifiNet(0), 2)
        );
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "Garage");
        assert!(v.rows.iter().any(|r| r.label == "New password"));
        assert_eq!(p.path(PageId::Car), UPDATES_PATH);
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(2)));
        let v = p.view(PageId::Car, &s, now);
        p.press(
            PageId::Car,
            &v,
            &Hit::Row(find(&v, "Add a network")),
            3,
            &client,
        );
        assert_eq!((p.screen(), p.depth(PageId::Car)), (CarScreen::WifiAdd, 2));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(3)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(5)));
        let v = p.view(PageId::Car, &s, now);
        p.press(
            PageId::Car,
            &v,
            &Hit::Row(find(&v, "Check and install")),
            4,
            &client,
        );
        assert_eq!(
            (p.screen(), p.depth(PageId::Car)),
            (CarScreen::UpdateConfirm, 1)
        );
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(find(&v, "Not now")), 1, &client),
            Moved::Closed(4)
        );
        assert_eq!(p.screen(), CarScreen::Updates);
        assert_eq!(p.back(PageId::Car), None);
        assert!(!client.snapshot().update.running()); // Cancel never starts the update.

        // A native route switch resets the anchored root and its trail.
        p.open_setup(&client);
        assert_eq!((p.screen(), p.depth(PageId::Car)), (CarScreen::Setup, 1));
        assert_eq!(p.path(PageId::Car), PageId::Car.path());
        let v = p.view(PageId::Car, &s, now);
        p.press(
            PageId::Car,
            &v,
            &Hit::Row(find(&v, "Wi-Fi networks")),
            6,
            &client,
        );
        assert_eq!(p.depth(PageId::Car), 2);
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(6)));
        p.reset(PageId::Car);
        assert_eq!((p.screen(), p.depth(PageId::Car)), (CarScreen::Root, 0));
        assert_eq!(p.path(PageId::Car), PageId::Car.path());
        assert_eq!(p.back(PageId::Car), None);
        let v = p.view(PageId::Car, &s, now);
        assert!(
            !v.rows
                .iter()
                .any(|r| r.act == Act::Car(CarScreen::Updates) || r.label == "Updates")
        );
    }

    #[test]
    fn updates_route_refresh_preserves_channel_progress_and_recovery() {
        use crate::client::UpdateJob;
        use carpi_proto::UpdateChannel;
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[]);
        s.settings = Some(crate::settings::sample());
        s.settings.as_mut().unwrap().update.channel = UpdateChannel::Stable;
        s.update_recovering = false;
        client.snapshot().update = UpdateJob {
            job: Some(42),
            stage: "download".into(),
            progress: Some((1, Some(4))),
            ..Default::default()
        };
        let ongoing = client.snapshot().update.clone();
        p.open_updates(&client);
        assert_eq!(client.snapshot().update, ongoing);
        s.update = ongoing;
        let v = p.view(PageId::Car, &s, Instant::now());
        assert_eq!(v.rows[find(&v, "Updating")].value, "Download 25 %");
        assert!(
            !v.rows
                .iter()
                .any(|r| r.act == Act::Car(CarScreen::UpdateConfirm))
        );
        let channel = find(&v, "Channel");
        let Some(Act::Patch(patch, false)) = v.side_act(Some(&Hit::Row(channel)), 1) else {
            panic!()
        };
        assert_eq!(
            patch.update.as_ref().unwrap().channel,
            Some(UpdateChannel::Beta)
        );
        assert!(matches!(v.rows[channel].act, Act::Patch(..)));
        s.update = UpdateJob::default();
        s.update_recovering = true;
        let v = p.view(PageId::Car, &s, Instant::now());
        assert_eq!(
            v.rows[find(&v, "Update status")].value,
            "Checking update status"
        );
        assert!(
            !v.rows
                .iter()
                .any(|r| r.act == Act::Car(CarScreen::UpdateConfirm))
        );
        // A fresh visit is still passive, even when there is no ongoing job.
        client.snapshot().update = UpdateJob::default();
        p.open_updates(&client);
        assert_eq!(client.snapshot().update, UpdateJob::default());
    }

    #[test]
    fn livi_fields_type_untrimmed_and_follow_auto() {
        use crate::keyboard::{Input, Prefs, Style};
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        assert!(p.type_field("Wi-Fi Password", "abc ", true, 63));
        assert!(p.typing_field());
        assert!(!p.keyboard().unwrap().touch);
        p.keyboard_input(Input::Char('d'), &client);
        assert_eq!(p.field_typed(), None);
        let done = p.keyboard().unwrap().rows()[3].len() - 1;
        p.keyboard_input(Input::Tap(3, done), &client);
        assert!(p.keyboard().is_none());
        assert_eq!(p.field_typed().as_deref(), Some("abc d"));
        assert_eq!(p.field_typed(), None);
        // Close types nothing.
        p.set_touch(true);
        assert!(p.type_field("Car Name", "x", false, 20));
        assert!(p.keyboard().unwrap().touch);
        p.keyboard_input(Input::Close, &client);
        assert!(!p.typing_field());
        assert_eq!(p.field_typed(), None);
        // A chosen layout beats the touch screen; Off leaves the field to LIVI.
        p.set_keyboard_prefs(Prefs {
            style: Style::Knob,
            ..Prefs::default()
        });
        assert!(p.type_field("Car Name", "x", false, 20));
        assert!(!p.keyboard().unwrap().touch);
        p.close_keyboard();
        p.set_keyboard_prefs(Prefs {
            style: Style::Off,
            ..Prefs::default()
        });
        assert!(!p.type_field("Car Name", "x", false, 20));
        assert!(p.keyboard().is_none());
    }

    #[test]
    fn the_grid_shows_the_page_and_why_values_are_missing() {
        let p = Pages::new(Saved::default(), None);
        let s = snap(&[
            ("engine.rpm", Some(Value::Num(900.0))),
            ("engine.coolant_c", None),
        ]);
        let v = p.view(PageId::Data, &s, Instant::now());
        assert_eq!(v.layout, Layout::Grid);
        let tabs: Vec<&str> = v.pills.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(tabs, ["Engine", "Drivetrain", "Body", "Electrical", "Edit"]);
        assert!(v.pills[0].active);
        assert_eq!(v.start, 0);
        assert_eq!((v.cols, v.cells.len()), (3, 9));
        assert_eq!(v.cells[0].value, "900");
        assert!(!v.cells[0].dim && v.cells[0].focus_at < 0);
        assert_eq!(v.cells[2].why, "no value");
        assert!(v.cells[2].dim);
        assert_eq!(v.cells[3].why, "not seen");
        assert!(v.cells[8].empty);
        assert!(v.note.is_empty());
    }

    #[test]
    fn the_dpad_steps_pages_and_tabs_pick_them() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[]);
        let now = Instant::now();
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(
            p.side(PageId::Data, &v, v.hit(4).as_ref(), 1, 4, &client),
            Moved::Focus(1)
        );
        let v = p.view(PageId::Data, &s, now);
        assert!(v.pills[1].active);
        assert_eq!(v.cells.len(), 16);
        p.press(PageId::Data, &v, &Hit::Pill(3), 3, &client);
        assert_eq!(p.board().shown, 3);
        p.side(PageId::Data, &v, None, 1, 3, &client);
        assert_eq!(p.board().shown, 3);
    }

    #[test]
    fn edit_picks_a_value_in_two_levels_and_comes_back() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[("engine.rpm", Some(Value::Num(900.0)))]);
        let now = Instant::now();
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(
            p.press(PageId::Data, &v, &Hit::Pill(4), 4, &client),
            Moved::Opened
        );
        assert_eq!(p.depth(PageId::Data), 1);
        let v = p.view(PageId::Data, &s, now);
        assert!(v.back && v.outlined);
        assert_eq!(v.title, "Edit");
        let Some(Hit::Cell(0)) = v.hit(v.start) else {
            panic!("starts on the first slot")
        };
        assert!(v.rows.iter().all(|r| !r.label.contains('.')));

        // Push the last slot: level 1, then Engine, then a value.
        p.press(PageId::Data, &v, &Hit::Cell(8), v.start, &client);
        assert_eq!(p.depth(PageId::Data), 2);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Slot 9");
        let engine = find(&v, "Engine");
        p.press(PageId::Data, &v, &Hit::Row(engine), 0, &client);
        assert_eq!(p.depth(PageId::Data), 3);
        let v = p.view(PageId::Data, &s, now);
        let boost = v
            .rows
            .iter()
            .position(|r| r.act == Act::Slots(SlotAct::Pick("engine.boost.actual".into())))
            .unwrap();
        assert_eq!(v.rows[boost].tag, "diag");
        assert_eq!(
            p.press(PageId::Data, &v, &Hit::Row(boost), 0, &client),
            Moved::Opened
        );
        assert_eq!(p.depth(PageId::Data), 1);
        assert_eq!(
            p.board().page().slots[8].as_deref(),
            Some("engine.boost.actual")
        );
        assert_eq!(p.recent(), ["engine.boost.actual"]);
        assert_eq!(
            p.watching(PageId::Data, &s),
            vec!["engine.boost.actual".to_string()]
        );
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.hit(v.start), Some(Hit::Cell(8)));
        assert_eq!(v.cells[8].why, "polling…");

        // Back leaves the editor.
        assert_eq!(p.back(PageId::Data), Some(Moved::Opened));
        assert_eq!(p.depth(PageId::Data), 0);
        assert_eq!(p.back(PageId::Data), None);
    }

    #[test]
    fn diagnostics_run_jobs_and_watch_the_module() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[("engine.maf_gs", Some(Value::Num(4.5)))]);
        let now = Instant::now();
        let v = p.view(PageId::Diagnostics, &s, now);
        assert!(v.note.is_empty());
        assert_eq!(v.pills[0].act, Act::Diag(DiagAction::InstallList));
        assert_eq!(v.rows[0].label, "Modules (not scanned yet)");
        assert_eq!(v.rows[1].label, "01  Engine");
        assert!(p.watching(PageId::Diagnostics, &s).is_empty());
        assert_eq!(
            p.press(PageId::Diagnostics, &v, &Hit::Row(1), 1, &client),
            Moved::Opened
        );
        let names = p.watching(PageId::Diagnostics, &s);
        assert_eq!(names.len(), 21);
        assert!(names.contains(&"engine.boost.actual".to_string()));
        let v = p.view(PageId::Diagnostics, &s, now);
        assert_eq!(v.title, "01  Engine");
        let maf = &v.rows[find(&v, "Air mass")];
        assert_eq!((maf.value.as_str(), maf.tag.as_str()), ("4.5 g/s", "003.2"));
        assert_eq!(v.rows[find(&v, "Boost")].value, "polling…");

        // Clear asks first, then sends confirm: true.
        let at = v
            .rows
            .iter()
            .position(|r| r.act == Act::ClearAsk(1))
            .unwrap();
        p.press(PageId::Diagnostics, &v, &Hit::Row(at), 9, &client);
        let v = p.view(PageId::Diagnostics, &s, now);
        let sure = &v.rows[find(&v, "Clear all of this module's codes?")];
        assert_eq!(
            sure.act,
            Act::Diag(DiagAction::ClearDtcs {
                module: 1,
                confirm: true
            })
        );
        let cancel = find(&v, "Keep them");
        p.press(PageId::Diagnostics, &v, &Hit::Row(cancel), 9, &client);
        let v = p.view(PageId::Diagnostics, &s, now);
        assert!(!v.rows.iter().any(|r| r.button == "Cancel"));
        let read = find(&v, "Read fault codes");
        p.press(PageId::Diagnostics, &v, &Hit::Row(read), 9, &client);
        assert_eq!(
            client.snapshot().diag.line(Some(1)).as_deref(),
            Some("Reading fault codes failed: not connected")
        );

        s.diag.started(3, DiagAction::ReadDtcs { module: 1 });
        s.diag.event(
            3,
            carpi_proto::JobEvent::Dtcs {
                module: 1,
                dtcs: vec![carpi_proto::Dtc {
                    code: 16684,
                    status: 0x23,
                }],
            },
        );
        let v = p.view(PageId::Diagnostics, &s, now);
        assert!(v.note.starts_with("Reading fault codes"));
        assert!(v.rows.iter().any(|r| r.label == "16684" && r.alert));
        let read = &v.rows[find(&v, "Read fault codes")];
        assert_eq!((read.value.as_str(), read.act.clone()), ("busy", Act::None));
        assert_eq!(p.back(PageId::Diagnostics), Some(Moved::Closed(1)));
        assert!(p.watching(PageId::Diagnostics, &s).is_empty());
        let v = p.view(PageId::Diagnostics, &s, now);
        assert!(v.pills.is_empty(), "no scan while a job runs");
        assert_eq!(v.rows[1].value, "1 fault");
    }

    #[test]
    fn the_light_screens_plot_step_and_edit_the_curve() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let curve = r#"{"source":"cluster","mode":"learned","trim":0.0,"points":[[0.00,12],[2.40,100]],"learned":[[0.60,18]]}"#;
        let mut s = snap(&[
            ("light.ambient", Some(Value::Num(1.2))),
            ("light.panel_pct", Some(Value::Num(44.0))),
            ("light.night", Some(Value::Bool(false))),
            ("light.night_source", Some(Value::Text("headlights".into()))),
            ("light.curve", Some(Value::Text(curve.into()))),
        ]);
        s.settings = Some(crate::settings::sample());
        let now = Instant::now();
        p.open(PageId::Car, "light");
        assert_eq!(p.path(PageId::Car), crate::LIGHT_PATH);
        assert_eq!(p.depth(PageId::Car), 1);
        let v = p.view(PageId::Car, &s, now);
        assert!(v.plot.is_none() && !v.custom);
        assert!(v.rows.iter().any(|r| r.value == "Day, by the headlights"));
        let b = find(&v, "Brightness now");
        assert_eq!(v.rows[b].kind, RowKind::Step);
        assert_eq!(
            v.side_act(Some(&Hit::Row(b)), 1),
            Some(&Act::Light(carpi_proto::LightAction::Nudge {
                delta_pct: 5
            }))
        );
        p.side(PageId::Car, &v, Some(&Hit::Row(b)), -1, b as i32, &client);
        assert_eq!(
            client.snapshot().settings_note.as_deref(),
            Some("not connected")
        );
        assert!(v.rows[find(&v, "Scroll trim")].checked);

        let c = find(&v, "Brightness curve");
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(c), 7, &client),
            Moved::Opened
        );
        let v = p.view(PageId::Car, &s, now);
        let plot = v.plot.as_ref().unwrap();
        assert_eq!(plot.curve, "M 0 880 L 960 0");
        assert_eq!(plot.now, Some((0.48, 0.44)));
        assert_eq!(plot.manual.len(), 3);
        assert!(v.custom);
        let second = v
            .rows
            .iter()
            .position(|r| r.act == Act::Car(CarScreen::Point(1)))
            .unwrap();
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(second), 4, &client),
            Moved::Opened
        );
        assert_eq!(p.path(PageId::Car), crate::LIGHT_PATH);
        let v = p.view(PageId::Car, &s, now);
        let Some(Act::Patch(patch, false)) = v.side_act(Some(&Hit::Row(0)), 1) else {
            panic!()
        };
        let points = patch.light.as_ref().unwrap().manual_curve.as_ref().unwrap();
        assert_eq!(points[1].pct, 55);
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(4)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(7)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(0)));
        assert_eq!(p.path(PageId::Car), "/carpi/car");

        p.car = CarScreen::Night;
        let v = p.view(PageId::Car, &s, now);
        let by = find(&v, "Decided by");
        assert_eq!(v.rows[by].value, "Headlights");
        let Some(Act::Patch(patch, false)) = v.side_act(Some(&Hit::Row(by)), -1) else {
            panic!()
        };
        assert_eq!(
            patch.light.as_ref().unwrap().dark_mode_source,
            Some(carpi_proto::DarkModeSource::Ambient)
        );
    }

    #[test]
    fn the_pin_is_entered_with_the_knob() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[]);
        s.settings = Some(crate::settings::sample());
        let now = Instant::now();
        p.car = CarScreen::Portal;
        p.trail = vec![0];
        let v = p.view(PageId::Car, &s, now);
        p.press(PageId::Car, &v, &Hit::Row(find(&v, "PIN")), 2, &client);
        assert_eq!(p.car, CarScreen::Pin);
        for d in [4, 7, 1, 1] {
            let v = p.view(PageId::Car, &s, now);
            assert!(v.pills.is_empty());
            let at = v
                .rows
                .iter()
                .position(|r| r.act == Act::PinDigit(d))
                .unwrap();
            p.press(PageId::Car, &v, &Hit::Row(at), 0, &client);
        }
        let v = p.view(PageId::Car, &s, now);
        assert!(v.rows.iter().any(|r| r.value == "••••"));
        assert!(!v.rows.iter().any(|r| r.value.contains("4711")));
        let del = find(&v, "Delete the last digit");
        p.press(PageId::Car, &v, &Hit::Row(del), 0, &client);
        let v = p.view(PageId::Car, &s, now);
        assert!(v.rows.iter().any(|r| r.value == "•••"));
        let save = find(&v, "Save this PIN");
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(save), 0, &client),
            Moved::Closed(2)
        );
        assert_eq!(p.car, CarScreen::Portal);
        assert!(p.pin.is_empty());
    }

    #[test]
    fn logging_is_reached_from_the_tab_and_edits_with_the_knob() {
        let client = Client::offline("test");
        client.snapshot().datalog = Some(crate::logging::tests::status());
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[
            ("engine.rpm", Some(Value::Num(900.0))),
            ("vehicle.speed_kmh", Some(Value::Num(50.0))),
        ]);
        s.datalog = Some(crate::logging::tests::status());
        s.settings = Some(crate::settings::sample());
        let now = Instant::now();
        p.car = CarScreen::Logs;
        p.trail = vec![0];
        let v = p.view(PageId::Car, &s, now);
        let at = find(&v, "Logging");
        assert_eq!(v.rows[at].value, "2 profiles");
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(at), 4, &client),
            Moved::Opened
        );
        assert_eq!((p.car, p.depth(PageId::Car)), (CarScreen::Logging, 2));
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "Logging");

        // Open Boost, then its channels, then the picker for a new one.
        let boost = find(&v, "Boost");
        p.press(PageId::Car, &v, &Hit::Row(boost), boost as i32, &client);
        assert_eq!((p.car, p.depth(PageId::Car)), (CarScreen::LogProfile, 3));
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "Boost");
        let ch = find(&v, "Channels");
        p.press(PageId::Car, &v, &Hit::Row(ch), ch as i32, &client);
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "Boost channels");
        let add = find(&v, "Add a channel");
        p.press(PageId::Car, &v, &Hit::Row(add), 0, &client);
        assert_eq!(p.depth(PageId::Car), 5);
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "New channel");
        p.press(PageId::Car, &v, &Hit::Row(find(&v, "Driving")), 0, &client);
        assert_eq!(p.depth(PageId::Car), 6);
        let v = p.view(PageId::Car, &s, now);
        let speed = v
            .rows
            .iter()
            .position(|r| r.act == Act::Slots(SlotAct::Pick("vehicle.speed_kmh".into())))
            .unwrap();
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(speed), 0, &client),
            Moved::Opened
        );
        assert_eq!(p.depth(PageId::Car), 4);
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.rows[3].label, "Speed");
        assert_eq!(v.hit(v.start), Some(Hit::Row(3)));
        assert_eq!(p.recent(), ["vehicle.speed_kmh"]);
        // Sent to carpid (offline here).
        assert_eq!(
            client.snapshot().datalog_note.as_deref(),
            Some("not connected")
        );

        // Back out: channels, profile, logging, logs, the tab.
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(ch as i32)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(boost as i32)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(4)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(0)));
        assert_eq!(p.back(PageId::Car), None);
        p.open(PageId::Car, "logging");
        assert_eq!(p.car, CarScreen::Logging);
    }

    #[test]
    fn capture_asks_carpid_and_toasts() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[]);
        s.settings = Some(crate::settings::sample());
        p.car = CarScreen::Logs;
        let v = p.view(PageId::Car, &s, Instant::now());
        let at = v.rows.iter().position(|r| r.act == Act::Capture).unwrap();
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(at), 0, &client),
            Moved::Stay
        );
        let s = client.snapshot();
        assert!(matches!(s.capture, Capture::Failed(_)));
        assert_eq!(
            s.toast.as_ref().unwrap().text,
            "Capture failed: not connected"
        );
    }
}
