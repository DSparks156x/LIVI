// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! Our pages: what each shows, and what pushing and the D-pad do (docs/PAGES.md 1).

use std::path::PathBuf;
use std::time::Instant;

use carpi_proto::{DiagAction, LightPatch, PinChange, Settings, SettingsPatch, Value};

use crate::PageId;
use crate::client::{Client, Snapshot};
use crate::config::{self, Saved};
use crate::defs::{DEFS, hidden};
use crate::light;
use crate::logging::Logging;
use crate::panels::{Memory, PanelScreen, Panels};
use crate::slots::{self, Picker, SlotAct, reading};
use crate::values::{Store, show};
use crate::view::{Act, CarScreen, DataTab, Hit, Layout, Row, RowKind, View};

/// The light model's curve, JSON for the plot: no use as a reading.
const CURVE: &str = "light.curve";

/// The native Settings slot that owns the bundled update screens.
pub const UPDATES_PATH: &str = "/settings/system/softwareUpdate";

mod data;
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
    /// The Data page's panels.
    panels: Panels,
    /// The Panels tab's screen.
    panel: PanelScreen,
    /// The Panels tab's value picker.
    pick: Picker,
    /// Graph history and peaks of the shown panel.
    mem: std::cell::RefCell<Memory>,
    /// Removing a widget or panel waits for a second push.
    dropping: bool,
    /// Clear all waits for a second push.
    clear_all: bool,
    /// Modules Clear all has still to clear (TP2), one job at a time.
    clearing: Vec<u8>,
    /// The module group last asked for, so it's asked once.
    asked: Option<(u8, u8)>,
    /// Sync-point groups a Forks widget has asked for (TP2, group), once each.
    synced: std::collections::BTreeSet<(u8, u8)>,
    /// The Data page's tab.
    tab: DataTab,
    /// The module open on the Faults tab.
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
    /// The Logs tab.
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
        for (name, unit) in &saved.units {
            crate::defs::learn_unit(name, unit);
        }
        Self {
            panels: saved.panels(),
            saved,
            file,
            panel: PanelScreen::View,
            pick: Picker::default(),
            mem: Default::default(),
            dropping: false,
            clear_all: false,
            clearing: Vec::new(),
            asked: None,
            synced: Default::default(),
            tab: DataTab::Panels,
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
                format!("Accel line {}", i + 1),
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

    pub fn panels(&self) -> &Panels {
        &self.panels
    }

    /// Shows panel `i` (the last when past the end).
    pub fn set_shown(&mut self, i: usize) {
        self.panels.shown = i.min(self.panels.panels.len() - 1);
    }

    /// Touch: a swipe or a tapped page dot flips the panel being viewed by `d`, as the knob does.
    pub fn flip(&mut self, page: PageId, d: i32, focus: i32, client: &Client) -> Moved {
        if page != PageId::Data
            || self.tab != DataTab::Panels
            || self.panel != crate::panels::PanelScreen::View
            || d == 0
        {
            return Moved::Stay;
        }
        self.run(
            page,
            Act::Panel(crate::panels::PanelAct::Step(d)),
            focus,
            client,
        )
    }

    /// A log mark, for graphs.
    pub fn mark(&mut self, n: u32, at: Instant) {
        self.mem.get_mut().marks.push((n, at));
    }

    /// Replaces the panels, unsaved (for pictures and tests).
    pub fn set_panels(&mut self, panels: Panels) {
        self.panels = panels.fixed();
    }

    pub fn recent(&self) -> &[String] {
        &self.saved.recent
    }

    pub fn depth(&self, page: PageId) -> usize {
        match page {
            PageId::Data => self.data_depth(),
            PageId::Car => {
                let picking =
                    if matches!(self.car, CarScreen::CellValue(..) | CarScreen::CellAdd(_)) {
                        self.reading.level()
                    } else {
                        0
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
                base.saturating_sub(self.car_root.depth()) + picking
            }
        }
    }

    /// Diag catalog names to keep subscribed, so carpid polls them: the shown panel's.
    pub fn watching(&self, page: PageId, snap: &Snapshot) -> Vec<String> {
        let _ = snap;
        match page {
            PageId::Data if self.tab == DataTab::Panels => {
                let mut out = self.panel_watch();
                out.extend(self.picker_watch());
                out
            }
            _ => self.picker_watch(),
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

    /// The page's first screen, or the screen `rest` names below it: `light` on the Car page,
    /// a tab (`logs`, `faults`) on the Data page.
    pub fn open(&mut self, page: PageId, rest: &str) {
        self.reset(page);
        let first = rest.split('/').next().unwrap_or("");
        match page {
            PageId::Data => {
                if let Some(t) = DataTab::ALL.into_iter().find(|t| t.name() == first) {
                    self.tab = t;
                }
            }
            PageId::Car if first == "light" => {
                self.car = CarScreen::Lighting;
                self.trail = vec![0; CarScreen::Lighting.depth()];
            }
            PageId::Car => {}
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
            PageId::Data => {
                self.panel = PanelScreen::View;
                self.pick = Picker::default();
                self.tab = DataTab::Panels;
                self.module = None;
                self.confirm = None;
                self.log.reset();
            }
            PageId::Car => {
                self.car = CarScreen::Root;
                self.car_root = CarScreen::Root;
                self.reading = Picker::default();
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
            PageId::Data => self.data_back(),
            PageId::Car => {
                if matches!(self.car, CarScreen::CellValue(..) | CarScreen::CellAdd(_))
                    && self.reading.back()
                {
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
                self.removing = false;
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
        if page == PageId::Data
            && let Some(moved) = self.data_run(&act, focus, client)
        {
            return moved;
        }
        let opened = match act {
            Act::None => false,
            Act::Slots(a) => return self.slots(page, &a, client),
            Act::Edit => false,
            Act::Module(i) => {
                self.module = Some(i);
                true
            }
            Act::Car(screen) => {
                self.car = screen;
                self.reading = Picker::default();
                screen != CarScreen::Root
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
            Act::ClearCancel | Act::ClearAll => false,
            Act::Light(action) => {
                client.light(action);
                false
            }
            Act::Advanced => false,
            Act::Log(_) | Act::Tab(_) | Act::LogPanel | Act::Panel(_) => false,
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
        if page == PageId::Data && self.tab == DataTab::Logs {
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
        self.panel_slots(a)
    }

    fn store(&mut self) {
        self.saved.panels = Some(self.panels.clone());
        let Some(file) = &self.file else { return };
        if let Err(e) = config::save(file, &self.saved) {
            eprintln!("[carpi-ui] can't save the pages: {e}");
        }
    }

    pub fn view(&self, page: PageId, snap: &Snapshot, now: Instant) -> View {
        match page {
            PageId::Data => self.data_view(snap, now),
            PageId::Car => tab::view(self, snap, now),
        }
    }
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

/// The Faults tab's module list, into `v` (which has the tabs): faults first once read, then
/// the rest in address order. Modules the install list says aren't coded are left out.
fn modules(mut v: View, snap: &Snapshot, clear_all: bool, clearing: usize) -> View {
    let diag = &snap.diag;
    v.note = diag_note(snap, None);
    let free = snap.connected && !diag.busy();
    if free {
        v.row(
            Row::new(RowKind::Button, "Scan", "")
                .button("Scan")
                .act(Act::Diag(DiagAction::Scan)),
        );
    }
    let mut list: Vec<(usize, &crate::diag::Installed)> = Vec::new();
    let all = diag.modules();
    for (i, m) in all.iter().enumerate() {
        if m.coded != Some(false) {
            list.push((i, m));
        }
    }
    let faults_of = |m: &crate::diag::Installed| diag.dtcs.get(&m.module.tp2).map_or(0, Vec::len);
    list.sort_by_key(|(_, m)| std::cmp::Reverse(faults_of(m)));
    for (i, m) in list {
        let faults = diag.dtcs.get(&m.module.tp2).map(Vec::len);
        let failed = diag.failed.contains(&m.module.address);
        let value = if diag.scanning == Some(m.module.address) {
            "Scanning…".to_string()
        } else if failed {
            "No answer".to_string()
        } else {
            match faults {
                Some(0) => "OK".to_string(),
                Some(1) => "1 fault".to_string(),
                Some(n) => format!("{n} faults"),
                None => String::new(),
            }
        };
        let mut row = Row::new(
            RowKind::Route,
            format!("{:02X}  {}", m.module.address, m.module.name),
            value,
        )
        .act(Act::Module(i));
        row.alert = faults.is_some_and(|n| n > 0) || failed;
        row.dimmed = diag.scanning.is_some() && faults.is_none() && !failed;
        v.row(row);
    }
    let any = diag.dtcs.values().any(|c| !c.is_empty());
    if clearing > 0 {
        v.row(Row::new(RowKind::Info, "Clear all", format!("{clearing} to go")).dimmed(true));
    } else if any && free {
        let mut row = Row::new(
            RowKind::Button,
            if clear_all {
                "Confirm clear all"
            } else {
                "Clear all"
            },
            "",
        )
        .button("Clear")
        .act(Act::ClearAll);
        row.alert = clear_all;
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
        return View::new(Layout::List, "Faults", true).finish();
    };
    let (addr, tp2) = (m.module.address, m.module.tp2);
    let note = diag_note(snap, Some(tp2));
    let mut v = View::new(Layout::List, format!("{addr:02X}  {}", m.module.name), true).note(note);
    let free = snap.connected && !diag.busy();

    match diag.dtcs.get(&tp2) {
        Some(codes) if codes.is_empty() => {
            v.row(Row::new(RowKind::Info, "No faults", ""));
        }
        Some(codes) => {
            for c in codes {
                let mut row = Row::new(
                    RowKind::Info,
                    c.code.clone(),
                    format!("Status {:02X}", c.status),
                );
                row.alert = true;
                row.mono = true;
                v.row(row);
            }
        }
        None => {
            v.row(job_row(
                "Read faults",
                "Read",
                free,
                Act::Diag(DiagAction::ReadDtcs { module: tp2 }),
            ));
        }
    }
    if diag.dtcs.get(&tp2).is_some_and(|c| !c.is_empty()) {
        let mut row = job_row(
            if confirm == Some(tp2) {
                "Confirm clear"
            } else {
                "Clear faults"
            },
            "Clear",
            free,
            if confirm == Some(tp2) {
                Act::Diag(DiagAction::ClearDtcs {
                    module: tp2,
                    confirm: true,
                })
            } else {
                Act::ClearAsk(tp2)
            },
        );
        row.alert = confirm == Some(tp2);
        v.row(row);
    }

    let _ = now;
    v.section("");
    match diag.identity.get(&tp2) {
        Some(id) => {
            let mut text = |label: &str, value: &Option<String>, mono: bool| {
                if let Some(x) = value.as_ref().filter(|x| !x.is_empty()) {
                    let mut r = Row::new(RowKind::Info, label, x.trim().to_string());
                    r.mono = mono;
                    v.row(r);
                }
            };
            text("Part no.", &id.part_no, false);
            text("Component", &id.component, false);
            text("Software", &id.sw_version, false);
            text("Coding", &id.coding, true);
            text("Workshop", &id.workshop, true);
        }
        None => {
            v.row(job_row(
                "Identity",
                "Read",
                free,
                Act::Diag(DiagAction::Identify { module: tp2 }),
            ));
        }
    }
    let mut a = Row::new(
        RowKind::Info,
        "Address",
        format!("{addr:02X} · TP2 {tp2:02X}"),
    );
    a.mono = true;
    v.row(a);
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
        assert!(v.rows.iter().any(|r| r.label == "Change password"));
        assert_eq!(p.path(PageId::Car), UPDATES_PATH);
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(2)));
        let v = p.view(PageId::Car, &s, now);
        p.press(
            PageId::Car,
            &v,
            &Hit::Row(find(&v, "Add network")),
            3,
            &client,
        );
        assert_eq!((p.screen(), p.depth(PageId::Car)), (CarScreen::WifiAdd, 2));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(3)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(5)));
        let v = p.view(PageId::Car, &s, now);
        p.press(PageId::Car, &v, &Hit::Row(find(&v, "Install")), 4, &client);
        assert_eq!(
            (p.screen(), p.depth(PageId::Car)),
            (CarScreen::UpdateConfirm, 1)
        );
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(find(&v, "Cancel")), 1, &client),
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
        assert_eq!(v.rows[find(&v, "Status")].value, "Checking");
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
    fn a_panel_takes_no_focus_and_the_knob_flips_panels() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[("engine.rpm", Some(Value::Num(900.0)))]);
        let now = Instant::now();
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.layout, Layout::Panel);
        assert_eq!(v.title, "Engine");
        let tabs: Vec<&str> = v.pills.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(tabs, ["Panels", "Logs", "Faults"]);
        assert_eq!((v.start, v.flip), (3, 3));
        let d = v.panel.as_ref().unwrap();
        assert_eq!(d.widgets[0].value, "900");
        assert_eq!(d.dots, p.panels().panels.len());
        assert!(p.panels().panels.last().unwrap().scratch);

        // The D-pad (and the knob, through livi-ui) on the panel flips panels.
        let hit = v.hit(v.flip);
        assert_eq!(
            p.side(PageId::Data, &v, hit.as_ref(), 1, 3, &client),
            Moved::Stay
        );
        assert_eq!(p.panels().shown, 1);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Drivetrain");
        // Push opens the menu; back comes to the panel.
        assert_eq!(
            p.press(PageId::Data, &v, &v.hit(v.flip).unwrap(), 3, &client),
            Moved::Opened
        );
        assert_eq!(p.depth(PageId::Data), 1);
        let v = p.view(PageId::Data, &s, now);
        assert!(v.rows.iter().any(|r| r.label == "Edit panel"));
        assert!(v.rows.iter().any(|r| r.label == "Log this panel"));
        assert_eq!(p.back(PageId::Data), Some(Moved::Closed(3)));
        assert_eq!(p.back(PageId::Data), None);
    }

    #[test]
    fn tabs_switch_with_the_dpad_and_push() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[]);
        let now = Instant::now();
        let v = p.view(PageId::Data, &s, now);
        p.side(PageId::Data, &v, Some(&Hit::Pill(0)), 1, 0, &client);
        let v = p.view(PageId::Data, &s, now);
        assert!(v.pills[1].active);
        assert_eq!(v.title, "Data");
        p.press(PageId::Data, &v, &Hit::Pill(0), 0, &client);
        let v = p.view(PageId::Data, &s, now);
        assert!(v.pills[0].active);
        assert_eq!(v.layout, Layout::Panel);
    }

    #[test]
    fn editing_adds_a_widget_restyles_and_removes_it() {
        use crate::panels::{PanelAct, Style};
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[("engine.coolant_c", Some(Value::Num(91.0)))]);
        let now = Instant::now();
        p.panels.panels.insert(
            0,
            crate::panels::Panel::new("Daily", crate::panels::Grid::G2x1, Vec::new()),
        );
        p.panels.shown = 0;
        let v = p.view(PageId::Data, &s, now);
        p.press(PageId::Data, &v, &v.hit(v.flip).unwrap(), 3, &client);
        let v = p.view(PageId::Data, &s, now);
        p.press(
            PageId::Data,
            &v,
            &Hit::Row(find(&v, "Edit panel")),
            0,
            &client,
        );
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Edit · Daily");
        assert_eq!(v.pills[0].label, "Grid 2×1");
        // Two free cells, the first focused.
        let d = v.panel.as_ref().unwrap();
        assert_eq!(d.empties.len(), 2);
        assert_eq!(v.hit(v.start), Some(Hit::Panel(0)));

        // Push the first: the picker, Engine, coolant.
        p.press(PageId::Data, &v, &Hit::Panel(0), v.start, &client);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Add value");
        p.press(PageId::Data, &v, &Hit::Row(find(&v, "Engine")), 0, &client);
        assert_eq!(p.depth(PageId::Data), 3);
        let v = p.view(PageId::Data, &s, now);
        p.press(PageId::Data, &v, &Hit::Row(find(&v, "Basics")), 0, &client);
        assert_eq!(p.depth(PageId::Data), 4);
        let v = p.view(PageId::Data, &s, now);
        let at = v
            .rows
            .iter()
            .position(|r| r.act == Act::Slots(SlotAct::Pick("engine.coolant_c".into())))
            .unwrap();
        assert!(matches!(
            p.press(PageId::Data, &v, &Hit::Row(at), 0, &client),
            Moved::Closed(_)
        ));
        assert_eq!(p.depth(PageId::Data), 1);
        assert_eq!(p.panels().panel().widgets[0].values, ["engine.coolant_c"]);
        assert_eq!(p.recent(), ["engine.coolant_c"]);

        // The widget: Style steps to Gauge, Size grows to 2×1.
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.panel.as_ref().unwrap().widgets[0].value, "91");
        p.press(PageId::Data, &v, &Hit::Panel(0), 0, &client);
        let v = p.view(PageId::Data, &s, now);
        assert!(v.panel.as_ref().unwrap().widgets[0].picked);
        let style = find(&v, "Style");
        p.side(PageId::Data, &v, Some(&Hit::Row(style)), 1, 0, &client);
        assert_eq!(p.panels().panel().widgets[0].style, Style::Gauge);
        let v = p.view(PageId::Data, &s, now);
        assert!(v.rows.iter().any(|r| r.label == "Peak"));
        let size = find(&v, "Size");
        p.side(PageId::Data, &v, Some(&Hit::Row(size)), 1, 0, &client);
        assert_eq!(p.panels().panel().widgets[0].w, 2);
        // Remove asks first.
        let v = p.view(PageId::Data, &s, now);
        let rm = find(&v, "Remove");
        assert_eq!(
            p.press(PageId::Data, &v, &Hit::Row(rm), 0, &client),
            Moved::Stay
        );
        let v = p.view(PageId::Data, &s, now);
        let rm = find(&v, "Confirm remove");
        assert!(matches!(
            p.press(PageId::Data, &v, &Hit::Row(rm), 0, &client),
            Moved::Closed(_)
        ));
        assert!(p.panels().panel().widgets.is_empty());
        let _ = PanelAct::Edit;
    }

    #[test]
    fn scratch_adds_values_and_saves_as_a_panel() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[("engine.rpm", Some(Value::Num(900.0)))]);
        let now = Instant::now();
        p.panels.shown = p.panels.scratch();
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Scratch");
        assert_eq!(v.panel.as_ref().unwrap().hint, "Push to add a value");
        // Empty: push goes straight to the picker.
        p.press(PageId::Data, &v, &v.hit(v.flip).unwrap(), 3, &client);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Add value");
        p.press(PageId::Data, &v, &Hit::Row(find(&v, "Engine")), 0, &client);
        let v = p.view(PageId::Data, &s, now);
        p.press(PageId::Data, &v, &Hit::Row(find(&v, "Basics")), 0, &client);
        let v = p.view(PageId::Data, &s, now);
        let at = v
            .rows
            .iter()
            .position(|r| r.act == Act::Slots(SlotAct::Pick("engine.rpm".into())))
            .unwrap();
        p.press(PageId::Data, &v, &Hit::Row(at), 0, &client);
        assert_eq!(p.depth(PageId::Data), 0);
        assert_eq!(p.panels().panel().widgets.len(), 1);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.panel.as_ref().unwrap().widgets[0].value, "900");
        // Save as panel: Scratch empties, the new one is shown.
        p.press(PageId::Data, &v, &v.hit(v.flip).unwrap(), 3, &client);
        let v = p.view(PageId::Data, &s, now);
        p.press(
            PageId::Data,
            &v,
            &Hit::Row(find(&v, "Save as panel")),
            0,
            &client,
        );
        assert_eq!(p.panels().panel().name, "Daily");
        assert_eq!(p.panels().panel().widgets.len(), 1);
        assert!(p.panels().panels.last().unwrap().widgets.is_empty());
    }

    #[test]
    fn touch_flips_panels_only_while_viewing() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let n = p.panels().panels.len();
        assert!(n > 1);
        assert_eq!(p.panels().shown, 0);
        p.flip(PageId::Data, 1, 0, &client);
        assert_eq!(p.panels().shown, 1);
        p.flip(PageId::Data, 10, 0, &client);
        assert_eq!(p.panels().shown, n - 1);
        p.flip(PageId::Car, -1, 0, &client);
        assert_eq!(p.panels().shown, n - 1);
        p.panel = crate::panels::PanelScreen::Menu;
        p.flip(PageId::Data, -1, 0, &client);
        assert_eq!(p.panels().shown, n - 1);
    }

    #[test]
    fn faults_scan_on_open_and_clear_with_a_confirm() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[]);
        let now = Instant::now();
        let v = p.view(PageId::Data, &s, now);
        p.press(PageId::Data, &v, &Hit::Pill(2), 2, &client);
        // Opening the tab scans (offline: it fails at once).
        assert_eq!(
            client.snapshot().diag.line(None).as_deref(),
            Some("Scan failed: not connected")
        );
        let v = p.view(PageId::Data, &s, now);
        assert!(v.pills[2].active);
        assert_eq!(v.rows[0].act, Act::Diag(DiagAction::Scan));
        assert_eq!(v.rows[1].label, "01  Engine");
        assert!(!v.rows.iter().any(|r| r.label.contains("Clear all")));

        // A scan in progress: the module it's on, one that didn't answer, faults first after.
        s.diag.started(3, DiagAction::Scan);
        s.diag.event(
            3,
            carpi_proto::JobEvent::Stage {
                stage: "failed".into(),
                message: "46 Central convenience".into(),
            },
        );
        s.diag.event(
            3,
            carpi_proto::JobEvent::Stage {
                stage: "module".into(),
                message: "22 AWD".into(),
            },
        );
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
        let v = p.view(PageId::Data, &s, now);
        assert!(
            v.rows.iter().all(|r| r.label != "Scan"),
            "no scan while one runs"
        );
        assert_eq!(v.rows[0].label, "01  Engine");
        assert_eq!(v.rows[0].value, "1 fault");
        assert_eq!(v.rows[find(&v, "22  AWD")].value, "Scanning…");
        let cc = &v.rows[find(&v, "46  Central convenience")];
        assert!(cc.alert && cc.value == "No answer");
        s.diag.event(
            3,
            carpi_proto::JobEvent::Finished {
                ok: true,
                message: "5 of 6 modules, 1 fault".into(),
            },
        );

        // The module: its fault, Clear asks first.
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(
            p.press(PageId::Data, &v, &Hit::Row(1), 4, &client),
            Moved::Opened
        );
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "01  Engine");
        assert!(v.rows.iter().any(|r| r.label == "16684" && r.alert));
        let clear = find(&v, "Clear faults");
        p.press(PageId::Data, &v, &Hit::Row(clear), 9, &client);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(
            v.rows[find(&v, "Confirm clear")].act,
            Act::Diag(DiagAction::ClearDtcs {
                module: 1,
                confirm: true
            })
        );
        assert_eq!(
            v.rows[find(&v, "Identity")].act,
            Act::Diag(DiagAction::Identify { module: 1 })
        );
        s.diag.identity.insert(
            1,
            crate::diag::Identity {
                part_no: Some("06F906056AK".into()),
                coding: Some("0403010A1C0700".into()),
                ..Default::default()
            },
        );
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.rows[find(&v, "Part no.")].value, "06F906056AK");
        assert!(v.rows[find(&v, "Coding")].mono);
        assert_eq!(p.back(PageId::Data), Some(Moved::Closed(4)));

        // Clear all asks, then works through the modules with faults (carpid's copy).
        client.snapshot().diag.dtcs = s.diag.dtcs.clone();
        let v = p.view(PageId::Data, &s, now);
        let all = find(&v, "Clear all");
        p.press(PageId::Data, &v, &Hit::Row(all), 9, &client);
        let v = p.view(PageId::Data, &s, now);
        let all = find(&v, "Confirm clear all");
        p.press(PageId::Data, &v, &Hit::Row(all), 9, &client);
        // Offline nothing goes; the queue waits for carpid.
        assert_eq!(p.clearing, [1]);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.rows[find(&v, "Clear all")].value, "1 to go");
        assert_eq!(p.back(PageId::Data), None);
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
        let by = find(&v, "Source");
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
        p.car = CarScreen::Remote;
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
        let del = find(&v, "Delete digit");
        p.press(PageId::Car, &v, &Hit::Row(del), 0, &client);
        let v = p.view(PageId::Car, &s, now);
        assert!(v.rows.iter().any(|r| r.value == "•••"));
        let save = find(&v, "Save PIN");
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(save), 0, &client),
            Moved::Closed(2)
        );
        assert_eq!(p.car, CarScreen::Remote);
        assert!(p.pin.is_empty());
    }

    #[test]
    fn logs_tab_edits_profiles_with_the_knob() {
        let client = Client::offline("test");
        client.snapshot().datalog = Some(crate::logging::tests::status());
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[
            ("engine.rpm", Some(Value::Num(900.0))),
            ("vehicle.speed_kmh", Some(Value::Num(50.0))),
        ]);
        s.datalog = Some(crate::logging::tests::status());
        let now = Instant::now();
        p.open(PageId::Data, "logs");
        let v = p.view(PageId::Data, &s, now);
        assert!(v.pills[1].active);
        assert_eq!(v.rows[find(&v, "Log this panel")].value, "Engine");
        let at = find(&v, "Profiles");
        assert_eq!(
            p.press(PageId::Data, &v, &Hit::Row(at), 7, &client),
            Moved::Opened
        );
        assert_eq!(p.depth(PageId::Data), 1);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Profiles");

        // Open Boost, then its channels, then the picker for a new one.
        let boost = find(&v, "Boost");
        p.press(PageId::Data, &v, &Hit::Row(boost), boost as i32, &client);
        assert_eq!(p.depth(PageId::Data), 2);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Boost");
        let ch = find(&v, "Channels");
        p.press(PageId::Data, &v, &Hit::Row(ch), ch as i32, &client);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.title, "Boost channels");
        let add = find(&v, "Add a channel");
        p.press(PageId::Data, &v, &Hit::Row(add), 0, &client);
        assert_eq!(p.depth(PageId::Data), 4);
        let v = p.view(PageId::Data, &s, now);
        p.press(PageId::Data, &v, &Hit::Row(find(&v, "Driving")), 0, &client);
        assert_eq!(p.depth(PageId::Data), 5);
        let v = p.view(PageId::Data, &s, now);
        p.press(PageId::Data, &v, &Hit::Row(find(&v, "Speed")), 0, &client);
        assert_eq!(p.depth(PageId::Data), 6);
        let v = p.view(PageId::Data, &s, now);
        let speed = v
            .rows
            .iter()
            .position(|r| r.act == Act::Slots(SlotAct::Pick("vehicle.speed_kmh".into())))
            .unwrap();
        assert_eq!(
            p.press(PageId::Data, &v, &Hit::Row(speed), 0, &client),
            Moved::Opened
        );
        assert_eq!(p.depth(PageId::Data), 3);
        let v = p.view(PageId::Data, &s, now);
        assert_eq!(v.rows[3].label, "Speed");
        assert_eq!(p.recent(), ["vehicle.speed_kmh"]);

        // Back out: channels, profile, profiles, the tab.
        assert_eq!(p.back(PageId::Data), Some(Moved::Closed(ch as i32)));
        assert_eq!(p.back(PageId::Data), Some(Moved::Closed(boost as i32)));
        assert_eq!(p.back(PageId::Data), Some(Moved::Closed(7)));
        assert_eq!(p.back(PageId::Data), None);
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
            "Debug logs failed: not connected"
        );
    }
}
