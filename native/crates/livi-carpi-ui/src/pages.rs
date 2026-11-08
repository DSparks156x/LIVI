// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The three pages: what each shows, and what pushing and the D-pad do (docs/PAGES.md 1).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use carpi_proto::{
    BusInfo, CurveMode, DarkModeSource, DiagAction, DisClaim, DisClock, DisMediaLayout, DisPage,
    DisPatch, DisUnits, LightAction, LightPatch, PinChange, Settings, SettingsPatch, Value,
};

use crate::PageId;
use crate::client::{Capture, Client, Snapshot};
use crate::config::{self, Saved};
use crate::defs::{BRIDGE, DEFS, hidden};
use crate::diag::{self, NO_POLL};
use crate::light::{self, STEP_AMBIENT, STEP_PCT};
use crate::logging::{self, Logging};
use crate::settings::{self as cfg, DIS_SLOTS};
use crate::slots::{self, Board, Editor, Picked, Picker, SlotAct, polled, reading, tile};
use crate::values::{NONE, Store, age, show};
use crate::view::{Act, CarScreen, Hit, Layout, Row, RowKind, View};

/// The light model's curve, JSON for the plot: no use as a reading.
const CURVE: &str = "light.curve";

/// Longest portal PIN the entry takes.
const PIN_MAX: usize = 8;

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
    trail: Vec<i32>,
    /// The portal PIN being entered.
    pin: String,
    /// The light page shows the manual curve's points.
    advanced: bool,
    /// Clearing this module's fault codes waits for a confirm.
    confirm: Option<u8>,
    /// The picker of a cluster display reading.
    reading: Picker,
    /// The Logging screens.
    log: Logging,
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
            trail: Vec::new(),
            pin: String::new(),
            advanced: false,
            confirm: None,
            reading: Picker::default(),
            log: Logging::default(),
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
                let picking =
                    matches!(self.car, CarScreen::Reading(_)) && self.reading.cat.is_some();
                let channel = match (&self.log.editor.picker, self.car) {
                    (Some(p), CarScreen::LogChannels) => 1 + usize::from(p.cat.is_some()),
                    _ => 0,
                };
                self.car.depth() + usize::from(picking) + channel
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
            (PageId::Car, CarScreen::Light | CarScreen::LightValues | CarScreen::Point(_)) => {
                crate::LIGHT_PATH
            }
            _ => page.path(),
        }
    }

    /// The page's first screen, or the screen `rest` names below it (`light` or `logging` on
    /// the Car page).
    pub fn open(&mut self, page: PageId, rest: &str) {
        self.reset(page);
        let screen = match rest.split('/').next() {
            Some("light") => CarScreen::Light,
            Some("logging") => CarScreen::Logging,
            _ => return,
        };
        if page == PageId::Car {
            self.car = screen;
            self.trail.push(0);
        }
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
                self.reading = Picker::default();
                self.log.reset();
                self.pin.clear();
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
                if matches!(self.car, CarScreen::Reading(_)) && self.reading.back() {
                    return Some(Moved::Opened);
                }
                if self.car == CarScreen::LogChannels && self.log.editor.back() {
                    return Some(Moved::Opened);
                }
                self.car = self.car.parent()?;
                self.log.removing = false;
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
            Act::ClearAsk(module) => {
                self.confirm = Some(module);
                false
            }
            Act::ClearCancel => false,
            Act::Light(action) => {
                client.light(action);
                false
            }
            Act::Advanced => {
                self.advanced = !self.advanced;
                false
            }
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
            return self.dis_pick(a, client);
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

    fn dis_pick(&mut self, a: &SlotAct, client: &Client) -> Moved {
        let CarScreen::Reading(at) = self.car else {
            return Moved::Stay;
        };
        match self.reading.apply(a, &slots::DIS, &self.saved.recent) {
            Picked::Moved => Moved::Opened,
            Picked::Stay => Moved::Stay,
            Picked::Chose(name) => {
                let Some(s) = client.snapshot().settings.clone() else {
                    return Moved::Stay;
                };
                if let Some(n) = &name {
                    slots::remember(&mut self.saved.recent, n);
                    self.store();
                }
                client.set(SettingsPatch {
                    dis_readings: Some(cfg::reading_set(&s.dis.readings, at, name.as_deref())),
                    ..Default::default()
                });
                self.reading = Picker::default();
                self.back(PageId::Car).unwrap_or(Moved::Stay)
            }
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
            PageId::Car => car(self.car, snap, now, &self.pin, self.advanced, self),
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

fn car(
    screen: CarScreen,
    snap: &Snapshot,
    now: Instant,
    pin: &str,
    advanced: bool,
    pages: &Pages,
) -> View {
    let note = if snap.connected {
        String::new()
    } else {
        problem(snap)
    };
    match screen {
        CarScreen::Root => car_root(snap, note),
        CarScreen::Buses => {
            let mut v = View::new(Layout::List, "Buses", true).note(note);
            bus_rows(&mut v, snap.buses.as_deref());
            v.finish()
        }
        CarScreen::Light => light_page(snap, advanced),
        CarScreen::LightValues => listed(snap, now, note, "Light values", &["light."]),
        CarScreen::Point(i) => curve_point(snap, i),
        CarScreen::Rnse => listed(snap, now, note, "RNS-E", &["rnse."]),
        CarScreen::Bridge => bridge(snap, now, note),
        CarScreen::Settings => settings(snap),
        CarScreen::Readings => readings(snap),
        CarScreen::Reading(i) => dis_slot(snap, now, i, pages),
        CarScreen::Bindings => bindings(snap),
        CarScreen::Binding(i) => binding(snap, i),
        CarScreen::Pin => pin_entry(snap, pin),
        CarScreen::Logging => logging::root(&pages.log, snap),
        CarScreen::LogProfile => logging::profile(&pages.log, snap),
        CarScreen::LogChannels => logging::channels(&pages.log, pages.recent(), snap, now),
    }
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

fn car_root(snap: &Snapshot, note: String) -> View {
    let mut v = View::new(Layout::List, "Car", false).note(note);
    let buses = match &snap.buses {
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|b| format!("{} {}", b.role, b.state.as_deref().unwrap_or("up")))
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    };
    let light = match flag(snap, "light.night") {
        Some(true) => "Night",
        Some(false) => "Day",
        None => "",
    };
    let rnse = match (flag(snap, "rnse.on_tv"), flag(snap, "rnse.radio_on")) {
        (Some(true), _) => "On TV",
        (_, Some(true)) => "On",
        (_, Some(false)) => "Off",
        _ => "",
    };
    let projection = match text_of(snap, "projection.kind") {
        Some("android_auto") => "Android Auto",
        Some("carplay") => "CarPlay",
        _ => "None",
    };
    for (label, value, screen) in [
        ("Light", light, CarScreen::Light),
        ("Buses", buses.as_str(), CarScreen::Buses),
        ("RNS-E", rnse, CarScreen::Rnse),
        ("Phone and bridge", projection, CarScreen::Bridge),
    ] {
        v.row(Row::new(RowKind::Route, label, value).act(Act::Car(screen)));
    }
    let tx = match &snap.settings {
        Some(s) if s.tx_enabled => "TX on",
        Some(_) => "TX off",
        None => "",
    };
    let logging = match &snap.datalog {
        Some(st) => match &st.running {
            Some(r) => format!("Logging {}", r.label),
            None => match st.profiles.len() {
                1 => "1 profile".to_string(),
                n => format!("{n} profiles"),
            },
        },
        None => String::new(),
    };
    v.row(Row::new(RowKind::Route, "Logging", logging).act(Act::Car(CarScreen::Logging)));
    v.row(Row::new(RowKind::Route, "Settings", tx).act(Act::Car(CarScreen::Settings)));
    v.section("Capture");
    let saving = snap.capture == Capture::Saving;
    if saving {
        v.row(Row::new(RowKind::Info, "Saving a capture", "…").dimmed(true));
    } else if snap.connected {
        v.row(
            Row::new(RowKind::Button, "Save the last 5 minutes", "")
                .button("Capture")
                .act(Act::Capture),
        );
    }
    match &snap.capture {
        Capture::Saved { label, summary } => {
            v.row(Row::new(
                RowKind::Info,
                format!("Last: {label}"),
                summary.clone(),
            ));
        }
        Capture::Failed(why) => {
            let mut row = Row::new(RowKind::Info, "Last capture failed", why.clone());
            row.alert = true;
            v.row(row);
        }
        Capture::Idle | Capture::Saving => {}
    }
    v.finish()
}

fn bus_rows(v: &mut View, buses: Option<&[BusInfo]>) {
    let Some(buses) = buses.filter(|b| !b.is_empty()) else {
        v.row(Row::new(RowKind::Info, "No bus data from carpid yet", "").dimmed(true));
        return;
    };
    for b in buses {
        v.section(format!("{} ({})", b.role, b.iface));
        v.row(Row::new(
            RowKind::Info,
            "State",
            b.state.as_deref().unwrap_or("no NM ring seen"),
        ));
        v.row(Row::new(RowKind::Info, "Frames", b.frames.to_string()));
        for m in &b.ring {
            let mut flags = Vec::new();
            if m.sleep_ind {
                flags.push("sleep ind");
            }
            if m.sleep_ack {
                flags.push("sleep ack");
            }
            if flags.is_empty() {
                flags.push("awake");
            }
            let mut row = Row::new(
                RowKind::Info,
                format!("Node {:02X}", m.node),
                format!(
                    "next {:02X} · {} · {}",
                    m.successor,
                    flags.join(", "),
                    age(Duration::from_millis(m.age_ms))
                ),
            );
            row.mono = true;
            v.row(row);
        }
    }
}

fn listed(snap: &Snapshot, now: Instant, note: String, title: &str, prefixes: &[&str]) -> View {
    let mut v = View::new(Layout::List, title, true).note(note);
    for p in prefixes {
        for n in names_under(p, &snap.values) {
            v.row(value_row(&n, snap, now));
        }
    }
    v.finish()
}

fn bridge(snap: &Snapshot, now: Instant, note: String) -> View {
    let mut v = View::new(Layout::List, "Phone and bridge", true).note(note);
    v.section("carpid");
    v.row(Row::new(
        RowKind::Info,
        "Daemon",
        snap.daemon.clone().unwrap_or_default(),
    ));
    let link = if snap.connected {
        "connected".to_string()
    } else {
        problem(snap)
    };
    v.row(Row::new(RowKind::Info, "Pages link", link));
    let mut any = false;
    for (p, title) in BRIDGE.iter().zip([
        "Projection",
        "Navigation",
        "Media",
        "Call",
        "Phone",
        "Focus",
    ]) {
        let names: Vec<String> = snap
            .values
            .under(p)
            .map(|(n, _)| n.to_string())
            .filter(|n| !n.ends_with("_base64"))
            .collect();
        if names.is_empty() {
            continue;
        }
        any = true;
        v.section(title);
        for n in names {
            v.row(value_row(&n, snap, now));
        }
    }
    if !any {
        v.section("Bridge");
        v.row(Row::new(RowKind::Info, "LIVI hasn't pushed anything to carpid", "").dimmed(true));
    }
    v.finish()
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

/// A choice the D-pad flips both ways, and push flips too: for settings with two values.
fn flip(label: &str, value: &str, act: Act) -> Row {
    Row::new(RowKind::Choice, label, value)
        .act(act.clone())
        .side(act.clone(), act)
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

fn settings(snap: &Snapshot) -> View {
    with_settings(snap, "Settings", |v, s| {
        v.row(toggle(
            "CAN TX",
            s.tx_enabled,
            patch(
                SettingsPatch {
                    tx_enabled: Some(!s.tx_enabled),
                    ..Default::default()
                },
                false,
            ),
        ));
        v.row(toggle(
            "Shut down when the car sleeps",
            s.power_shutdown,
            patch(
                SettingsPatch {
                    power_shutdown: Some(!s.power_shutdown),
                    ..Default::default()
                },
                false,
            ),
        ));
        v.section("Cluster display");
        v.row(toggle(
            "Cluster display",
            s.dis.enabled,
            patch(
                SettingsPatch {
                    dis_enabled: Some(!s.dis.enabled),
                    ..Default::default()
                },
                false,
            ),
        ));
        v.row(
            Row::new(
                RowKind::Route,
                "Readings",
                format!("{} of {DIS_SLOTS}", s.dis.readings.len()),
            )
            .act(Act::Car(CarScreen::Readings)),
        );
        dis_rows(v, &s.dis);
        v.section("Keys and portal");
        v.row(
            Row::new(
                RowKind::Route,
                "Key bindings",
                format!("{} rows", s.bindings.len()),
            )
            .act(Act::Car(CarScreen::Bindings)),
        );
        let set = if s.portal_pin_set { "set" } else { "not set" };
        v.row(Row::new(RowKind::Route, "Portal PIN", set).act(Act::Car(CarScreen::Pin)));
        if s.portal_pin_set {
            v.row(
                Row::new(RowKind::Button, "Clear the portal PIN", "")
                    .button("Clear")
                    .act(patch(
                        SettingsPatch {
                            portal_pin: Some(PinChange::Clear),
                            ..Default::default()
                        },
                        false,
                    )),
            );
        }
    })
}

fn dis_patch(p: DisPatch) -> Act {
    patch(
        SettingsPatch {
            dis: Some(p),
            ..Default::default()
        },
        false,
    )
}

/// The rest of `[dis]`, edited in place: claim, default page, auto-shows, media layout, cover
/// flash.
fn dis_rows(v: &mut View, d: &carpi_proto::DisSettings) {
    let (claim, other) = match d.claim {
        DisClaim::Always => ("Always", DisClaim::Tab),
        DisClaim::Tab => ("Cluster menu tab", DisClaim::Always),
    };
    v.row(flip(
        "Show the page",
        claim,
        dis_patch(DisPatch {
            claim: Some(other),
            ..Default::default()
        }),
    ));
    const PAGES: [(DisPage, &str); 3] = [
        (DisPage::Readings, "Readings"),
        (DisPage::Accel, "Accel"),
        (DisPage::Media, "Media"),
    ];
    let at = PAGES
        .iter()
        .position(|p| p.0 == d.default_page)
        .unwrap_or(0);
    let page = |step: usize| {
        dis_patch(DisPatch {
            default_page: Some(PAGES[(at + step) % PAGES.len()].0),
            ..Default::default()
        })
    };
    v.row(
        Row::new(RowKind::Choice, "Default page", PAGES[at].1)
            .act(page(1))
            .side(page(PAGES.len() - 1), page(1)),
    );
    for (label, on, set) in [
        (
            "Take over when a route starts",
            d.claim_nav,
            DisPatch {
                claim_nav: Some(!d.claim_nav),
                ..Default::default()
            },
        ),
        (
            "Take over on a call",
            d.claim_phone,
            DisPatch {
                claim_phone: Some(!d.claim_phone),
                ..Default::default()
            },
        ),
        (
            "Take over on a new track",
            d.claim_media,
            DisPatch {
                claim_media: Some(!d.claim_media),
                ..Default::default()
            },
        ),
    ] {
        v.row(toggle(label, on, dis_patch(set)));
    }
    let (layout, other) = match d.media_layout {
        DisMediaLayout::Stack => ("Text", DisMediaLayout::Art),
        DisMediaLayout::Art => ("Cover + text", DisMediaLayout::Stack),
    };
    v.row(flip(
        "Media layout",
        layout,
        dis_patch(DisPatch {
            media_layout: Some(other),
            ..Default::default()
        }),
    ));
    const UNITS: [(DisUnits, &str); 3] = [
        (DisUnits::Auto, "As the phone"),
        (DisUnits::Metric, "Metric"),
        (DisUnits::Imperial, "Imperial"),
    ];
    let u = UNITS.iter().position(|x| x.0 == d.units).unwrap_or(0);
    let units = |step: usize| {
        dis_patch(DisPatch {
            units: Some(UNITS[(u + step) % UNITS.len()].0),
            ..Default::default()
        })
    };
    v.row(
        Row::new(RowKind::Choice, "Units", UNITS[u].1)
            .act(units(1))
            .side(units(UNITS.len() - 1), units(1)),
    );
    const CLOCKS: [(DisClock, &str); 3] = [
        (DisClock::Auto, "Auto"),
        (DisClock::H12, "12 h"),
        (DisClock::H24, "24 h"),
    ];
    let c = CLOCKS.iter().position(|x| x.0 == d.clock).unwrap_or(0);
    let clock = |step: usize| {
        dis_patch(DisPatch {
            clock: Some(CLOCKS[(c + step) % CLOCKS.len()].0),
            ..Default::default()
        })
    };
    v.row(
        Row::new(RowKind::Choice, "ETA clock", CLOCKS[c].1)
            .act(clock(1))
            .side(clock(CLOCKS.len() - 1), clock(1)),
    );
    let flash = |s: f64| {
        dis_patch(DisPatch {
            cover_flash_s: Some(s),
            ..Default::default()
        })
    };
    let shown = if d.cover_flash_s <= 0.0 {
        "Off".to_string()
    } else {
        format!("{} s", d.cover_flash_s)
    };
    // A choice like Units: push (or a tap) steps on and wraps to Off; the
    // D-pad steps both ways. As a Step row a push did nothing (Drew).
    let at = cfg::COVER_FLASH
        .iter()
        .rposition(|x| *x <= d.cover_flash_s)
        .unwrap_or(0);
    let next = cfg::COVER_FLASH[(at + 1) % cfg::COVER_FLASH.len()];
    v.row(
        Row::new(RowKind::Choice, "Cover flash", shown)
            .act(flash(next))
            .side(
                flash(cfg::flash_step(d.cover_flash_s, -1)),
                flash(cfg::flash_step(d.cover_flash_s, 1)),
            ),
    );
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

/// PLAN.md 2.6: the plot and the brightness first, the options after, manual editing behind a
/// switch. The D-pad steps the panel and flips the choices in place.
fn light_page(snap: &Snapshot, advanced: bool) -> View {
    let settings = snap.settings.as_ref().map(|s| &s.light);
    let ambient = num(snap, "light.ambient");
    let pct = num(snap, "light.panel_pct");
    let curve = text_of(snap, CURVE).and_then(light::parse);
    let auto = settings.is_none_or(|l| l.auto_brightness);
    let mut v = View::new(Layout::List, "Light", true).note(settings_note(snap));
    v.plot = Some(light::plot(
        curve.as_ref(),
        settings,
        ambient,
        pct,
        advanced,
    ));

    let nudge = |delta_pct| Act::Light(LightAction::Nudge { delta_pct });
    let panel = pct.map_or_else(|| NONE.to_string(), |p| format!("{p:.0} %"));
    if snap.connected && auto && pct.is_some() {
        v.row(
            Row::new(RowKind::Step, "Panel brightness", panel)
                .side(nudge(-(STEP_PCT as i8)), nudge(STEP_PCT as i8)),
        );
    } else {
        let why = if auto {
            panel
        } else {
            format!("{panel}, the car's dimming")
        };
        v.row(Row::new(RowKind::Info, "Panel brightness", why));
    }
    v.row(Row::new(
        RowKind::Info,
        "Ambient",
        ambient.map_or_else(|| NONE.to_string(), ambient_text),
    ));
    let night = match flag(snap, "light.night") {
        Some(true) => "Night",
        Some(false) => "Day",
        None => NONE,
    };
    let dark = match text_of(snap, "light.night_source") {
        Some(by) => format!("{night}, by the {by}"),
        None => night.to_string(),
    };
    v.row(Row::new(RowKind::Info, "Now", dark));

    let Some(l) = settings else {
        v.row(Row::new(RowKind::Info, "Waiting for carpid's settings", "").dimmed(true));
        return v.finish();
    };
    v.section("Brightness");
    v.row(toggle(
        "Auto brightness",
        l.auto_brightness,
        light_patch(
            LightPatch {
                auto_brightness: Some(!l.auto_brightness),
                ..Default::default()
            },
            false,
        ),
    ));
    v.row(toggle(
        "Scroll trim",
        l.scroll_trim,
        light_patch(
            LightPatch {
                scroll_trim: Some(!l.scroll_trim),
                ..Default::default()
            },
            false,
        ),
    ));
    let source = |s| {
        light_patch(
            LightPatch {
                ambient_source: Some(s),
                ..Default::default()
            },
            false,
        )
    };
    v.row(
        Row::new(
            RowKind::Choice,
            "Light sensor",
            capital(light::source_name(l.ambient_source)),
        )
        .act(source(light::next_source(l.ambient_source)))
        .side(
            source(light::prev_source(l.ambient_source)),
            source(light::next_source(l.ambient_source)),
        ),
    );
    let other = match l.curve_mode {
        CurveMode::Manual => CurveMode::Learned,
        CurveMode::Learned => CurveMode::Manual,
    };
    v.row(flip(
        "Curve",
        &capital(light::mode_name(l.curve_mode)),
        light_patch(
            LightPatch {
                curve_mode: Some(other),
                ..Default::default()
            },
            false,
        ),
    ));
    let learned = l
        .learned
        .iter()
        .filter(|p| p.source == l.ambient_source)
        .count();
    if snap.connected && learned > 0 {
        v.row(
            Row::new(RowKind::Button, "Learned points", learned.to_string())
                .button("Reset")
                .act(Act::Light(LightAction::ResetLearned { source: None })),
        );
    }
    v.row(toggle("Edit the manual curve", advanced, Act::Advanced));
    if advanced {
        v.section("Manual curve");
        for (i, p) in l.manual_curve.iter().enumerate() {
            v.row(
                Row::new(
                    RowKind::Route,
                    format!("Point {}", i + 1),
                    format!("sensor {:.0} → {} %", light::raw(p.ambient), p.pct),
                )
                .act(Act::Car(CarScreen::Point(i))),
            );
        }
        if let Some(here) = ambient {
            let level = pct.map_or(50, |p| p.round().clamp(0.0, 100.0) as u8);
            v.row(
                Row::new(RowKind::Button, "Add a point here", format!("{level} %"))
                    .button("Add")
                    .act(light_patch(
                        LightPatch {
                            manual_curve: Some(light::added(&l.manual_curve, here, level)),
                            ..Default::default()
                        },
                        false,
                    )),
            );
        }
    }

    v.section("Dark mode");
    let (by, other) = match l.dark_mode_source {
        DarkModeSource::Headlights => ("Headlights", DarkModeSource::Ambient),
        DarkModeSource::Ambient => ("Light sensor", DarkModeSource::Headlights),
    };
    v.row(flip(
        "Decided by",
        by,
        light_patch(
            LightPatch {
                dark_mode_source: Some(other),
                ..Default::default()
            },
            false,
        ),
    ));
    v.row(Row::new(
        RowKind::Info,
        "Night below",
        format!(
            "{} ± {:.2}",
            ambient_text(l.night_threshold),
            l.night_hysteresis
        ),
    ));
    let clock = match (l.latitude, l.longitude) {
        (Some(la), Some(lo)) => format!("sunset to sunrise at {la:.1}, {lo:.1}"),
        _ => format!("night {} to {}", l.night_from, l.day_from),
    };
    v.row(Row::new(RowKind::Info, "Without either", clock));
    v.section("");
    v.row(Row::new(RowKind::Route, "All light values", "").act(Act::Car(CarScreen::LightValues)));
    v.finish()
}

fn curve_point(snap: &Snapshot, at: usize) -> View {
    with_settings(snap, &format!("Curve point {}", at + 1), |v, s| {
        let c = &s.light.manual_curve;
        let Some(p) = c.get(at) else {
            v.row(Row::new(RowKind::Info, "This point is gone", "").dimmed(true));
            return;
        };
        let edit = |pct: i16, ambient: f64| {
            light_patch(
                LightPatch {
                    manual_curve: Some(light::moved(c, at, pct, ambient)),
                    ..Default::default()
                },
                false,
            )
        };
        v.row(
            Row::new(RowKind::Step, "Panel", format!("{} %", p.pct))
                .side(edit(-STEP_PCT, 0.0), edit(STEP_PCT, 0.0)),
        );
        v.row(
            Row::new(RowKind::Step, "Ambient", ambient_text(p.ambient))
                .side(edit(0, -STEP_AMBIENT), edit(0, STEP_AMBIENT)),
        );
        if c.len() > 2 {
            v.row(
                Row::new(RowKind::Button, "Remove this point", "")
                    .button("Remove")
                    .act(light_patch(
                        LightPatch {
                            manual_curve: Some(light::removed(c, at)),
                            ..Default::default()
                        },
                        true,
                    )),
            );
        }
    })
}

/// Left column top to bottom, then right.
fn slot_name(i: usize) -> String {
    let half = DIS_SLOTS / 2;
    if i < half {
        format!("Left {}", i + 1)
    } else {
        format!("Right {}", i - half + 1)
    }
}

fn readings(snap: &Snapshot) -> View {
    with_settings(snap, "Cluster readings", |v, s| {
        let list = &s.dis.readings;
        for i in 0..(list.len() + 1).min(DIS_SLOTS) {
            let value = list
                .get(i)
                .map_or_else(|| "empty".to_string(), |r| slots::label(&r.key));
            v.row(
                Row::new(RowKind::Route, slot_name(i), value).act(Act::Car(CarScreen::Reading(i))),
            );
        }
    })
}

fn dis_slot(snap: &Snapshot, now: Instant, at: usize, pages: &Pages) -> View {
    let Some(s) = &snap.settings else {
        return with_settings(snap, &slot_name(at), |_, _| {});
    };
    let current = s.dis.readings.get(at).map(|r| r.key.as_str());
    let mut v = pages.reading.view(
        &slot_name(at),
        current,
        pages.recent(),
        snap,
        now,
        &slots::DIS,
    );
    v.note = settings_note(snap);
    v
}

fn bindings(snap: &Snapshot) -> View {
    with_settings(snap, "Key bindings", |v, s| {
        let mut table = "";
        for (i, b) in s.bindings.iter().enumerate() {
            if b.table != table {
                v.section(b.table.replace('_', " "));
                table = &b.table;
            }
            v.row(
                Row::new(
                    RowKind::Route,
                    format!("{} {}", b.key, b.gesture),
                    b.action.replace('_', " "),
                )
                .act(Act::Car(CarScreen::Binding(i))),
            );
        }
    })
}

fn binding(snap: &Snapshot, at: usize) -> View {
    let title = snap
        .settings
        .as_ref()
        .and_then(|s| s.bindings.get(at))
        .map_or_else(|| "Key binding".to_string(), cfg::binding_label);
    with_settings(snap, &title, |v, s| {
        let Some(b) = s.bindings.get(at) else {
            v.row(Row::new(RowKind::Info, "This row is gone", "").dimmed(true));
            return;
        };
        let mut actions: Vec<&str> = cfg::actions(&b.table).to_vec();
        if !actions.contains(&b.action.as_str()) {
            actions.insert(0, &b.action);
        }
        for a in actions {
            let set = SettingsPatch {
                bindings: Some(cfg::rebound(&s.bindings, at, a)),
                ..Default::default()
            };
            let at = v.row(
                Row::new(RowKind::Info, a.replace('_', " "), "")
                    .act(patch(set, true))
                    .checked(a == b.action),
            );
            if a == b.action {
                v.start = at;
            }
        }
    })
}

fn pin_entry(snap: &Snapshot, pin: &str) -> View {
    let mut v = View::new(Layout::List, "Portal PIN", true).note(settings_note(snap));
    if !pin.is_empty() {
        v.pill("Delete", false, Act::PinDelete);
        if snap.connected {
            v.pill("Save", true, Act::PinSave);
        }
    }
    let shown = if pin.is_empty() {
        "push digits below".to_string()
    } else {
        "•".repeat(pin.len())
    };
    v.row(Row::new(RowKind::Info, "New PIN", shown).dimmed(pin.is_empty()));
    v.section("Digits");
    if pin.len() < PIN_MAX {
        for d in (1..=9).chain([0]) {
            v.row(
                Row::new(RowKind::Button, d.to_string(), "")
                    .button("Add")
                    .act(Act::PinDigit(d)),
            );
        }
    }
    v.start = v.first_body() + 1;
    v.finish()
}

#[cfg(test)]
mod tests {
    use carpi_proto::{Reading, RingMember};

    use super::*;

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
    fn the_car_page_reads_buses_light_and_bridge() {
        let mut s = snap(&[
            ("light.night", Some(Value::Bool(true))),
            ("rnse.on_tv", Some(Value::Bool(true))),
            ("projection.kind", Some(Value::Text("android_auto".into()))),
            ("nav.icon_png_base64", Some(Value::Text("AAAA".into()))),
            ("nav.road", Some(Value::Text("A9".into()))),
        ]);
        s.buses = Some(vec![BusInfo {
            role: "ican".into(),
            iface: "can0".into(),
            state: Some("awake".into()),
            frames: 1234,
            ring: vec![RingMember {
                node: 0x20,
                successor: 0x21,
                sleep_ind: false,
                sleep_ack: false,
                age_ms: 80,
            }],
        }]);
        let p = Pages::new(Saved::default(), None);
        let now = Instant::now();
        let at = |screen| car(screen, &s, now, "", false, &p);
        let v = at(CarScreen::Root);
        let values: Vec<&str> = v.rows.iter().map(|r| r.value.as_str()).collect();
        assert!(values.contains(&"ican awake"));
        assert!(values.contains(&"Night"));
        assert!(values.contains(&"On TV"));
        assert!(values.contains(&"Android Auto"));
        let v = at(CarScreen::Buses);
        assert!(v.rows.iter().any(|r| r.value == "1234"));
        assert!(v.rows.iter().any(|r| r.label == "Node 20"));
        let v = at(CarScreen::Bridge);
        let road = &v.rows[find(&v, "Road")];
        assert_eq!(road.value, "A9");
        assert!(!v.rows.iter().any(|r| r.label.contains('.')));
        let v = at(CarScreen::Settings);
        assert!(
            v.rows
                .iter()
                .any(|r| r.label == "Waiting for carpid's settings")
        );
        let v = at(CarScreen::Rnse);
        assert_eq!(v.rows[find(&v, "RNS-E on")].value, "not seen since start");
    }

    #[test]
    fn settings_edit_through_carpid() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[("engine.rpm", Some(Value::Num(800.0)))]);
        s.settings = Some(cfg::sample());
        let now = Instant::now();
        let v = car(CarScreen::Settings, &s, now, "", false, &p);
        let tx = &v.rows[find(&v, "CAN TX")];
        assert!(!tx.checked);
        let Act::Patch(patch, false) = &tx.act else {
            panic!("{:?}", tx.act)
        };
        assert_eq!(patch.tx_enabled, Some(true));
        assert!(
            !v.rows.iter().any(|r| r.button == "Clear"),
            "no PIN to clear"
        );

        // Settings, then bindings, then one row: three deep, and back twice after picking.
        let v = p.view(PageId::Car, &s, now);
        p.press(PageId::Car, &v, &Hit::Row(find(&v, "Settings")), 3, &client);
        let v = p.view(PageId::Car, &s, now);
        let at = find(&v, "Key bindings");
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(at), 5, &client),
            Moved::Opened
        );
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.rows[0].kind, RowKind::Section);
        assert_eq!(v.rows[3].value, "up");
        p.press(PageId::Car, &v, &Hit::Row(3), 7, &client);
        assert_eq!(p.depth(PageId::Car), 3);
        let v = p.view(PageId::Car, &s, now);
        let select = find(&v, "select");
        let Act::Patch(patch, true) = &v.rows[select].act else {
            panic!("picking goes back")
        };
        assert_eq!(patch.bindings.as_ref().unwrap()[1].action, "select");
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(select), 9, &client),
            Moved::Closed(7)
        );
        assert_eq!(p.depth(PageId::Car), 2);
        assert_eq!(
            client.snapshot().settings_note.as_deref(),
            Some("not connected")
        );
        p.reset(PageId::Car);
        assert_eq!(p.depth(PageId::Car), 0);
    }

    #[test]
    fn dis_readings_use_the_picker() {
        let client = Client::offline("test");
        client.snapshot().settings = Some(cfg::sample());
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[("nav.road", Some(Value::Text("A9".into())))]);
        s.settings = Some(cfg::sample());
        let now = Instant::now();
        p.car = CarScreen::Readings;
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.rows[0].value, "Coolant");
        p.press(PageId::Car, &v, &Hit::Row(0), 0, &client);
        assert_eq!(p.car, CarScreen::Reading(0));
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "Left 1");
        let phone = find(&v, "Phone & nav");
        p.press(PageId::Car, &v, &Hit::Row(phone), 0, &client);
        assert_eq!(p.depth(PageId::Car), 4);
        let v = p.view(PageId::Car, &s, now);
        let road = find(&v, "Road");
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(road), 0, &client),
            Moved::Closed(0)
        );
        assert_eq!(p.car, CarScreen::Readings);
        assert_eq!(p.recent(), ["nav.road"]);
    }

    #[test]
    fn the_light_page_plots_steps_and_edits_the_curve() {
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
        let now = Instant::now();
        p.open(PageId::Car, "light");
        assert_eq!(p.path(PageId::Car), crate::LIGHT_PATH);
        assert_eq!(p.depth(PageId::Car), 1);
        let v = p.view(PageId::Car, &s, now);
        let plot = v.plot.as_ref().unwrap();
        assert_eq!(plot.curve, "M 0 880 L 960 0");
        assert_eq!(plot.now, Some((0.48, 0.44)));
        assert!(v.pills.is_empty());
        assert!(v.rows.iter().any(|r| r.value == "Day, by the headlights"));
        let b = find(&v, "Panel brightness");
        assert_eq!(v.rows[b].kind, RowKind::Step);
        assert_eq!(
            v.side_act(Some(&Hit::Row(b)), 1),
            Some(&Act::Light(LightAction::Nudge { delta_pct: 5 }))
        );
        p.side(PageId::Car, &v, Some(&Hit::Row(b)), -1, b as i32, &client);
        assert_eq!(
            client.snapshot().settings_note.as_deref(),
            Some("not connected")
        );

        s.settings = Some(cfg::sample());
        let v = p.view(PageId::Car, &s, now);
        assert!(v.rows[find(&v, "Scroll trim")].checked);
        let by = &v.rows[find(&v, "Decided by")];
        assert_eq!(
            (by.kind, by.value.as_str()),
            (RowKind::Choice, "Headlights")
        );
        let Some(Act::Patch(patch, false)) =
            v.side_act(Some(&Hit::Row(find(&v, "Decided by"))), -1)
        else {
            panic!()
        };
        assert_eq!(
            patch.light.as_ref().unwrap().dark_mode_source,
            Some(DarkModeSource::Ambient)
        );
        let edit = v.rows.iter().position(|r| r.act == Act::Advanced).unwrap();
        p.press(PageId::Car, &v, &Hit::Row(edit), 0, &client);
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.plot.as_ref().unwrap().manual.len(), 3);
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
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(0)));
        assert_eq!(p.path(PageId::Car), "/carpi/car");
    }

    #[test]
    fn the_pin_is_entered_with_the_knob() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[]);
        s.settings = Some(cfg::sample());
        let now = Instant::now();
        p.car = CarScreen::Pin;
        let v = p.view(PageId::Car, &s, now);
        assert!(v.pills.is_empty());
        for d in [4, 7, 1, 1] {
            let v = p.view(PageId::Car, &s, now);
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
        p.press(PageId::Car, &v, &Hit::Pill(0), 0, &client);
        let v = p.view(PageId::Car, &s, now);
        assert!(v.rows.iter().any(|r| r.value == "•••"));
        assert!(matches!(
            p.press(PageId::Car, &v, &Hit::Pill(1), 0, &client),
            Moved::Closed(_)
        ));
        assert_eq!(p.car, CarScreen::Settings);
        assert!(p.pin.is_empty());
    }

    #[test]
    fn logging_is_reached_from_the_car_page_and_edits_with_the_knob() {
        let client = Client::offline("test");
        client.snapshot().datalog = Some(crate::logging::tests::status());
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[
            ("engine.rpm", Some(Value::Num(900.0))),
            ("vehicle.speed_kmh", Some(Value::Num(50.0))),
        ]);
        s.datalog = Some(crate::logging::tests::status());
        let now = Instant::now();
        let v = p.view(PageId::Car, &s, now);
        let at = find(&v, "Logging");
        assert_eq!(v.rows[at].value, "2 profiles");
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(at), 4, &client),
            Moved::Opened
        );
        assert_eq!((p.car, p.depth(PageId::Car)), (CarScreen::Logging, 1));
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "Logging");

        // Open Boost, then its channels, then the picker for a new one.
        let boost = find(&v, "Boost");
        p.press(PageId::Car, &v, &Hit::Row(boost), boost as i32, &client);
        assert_eq!((p.car, p.depth(PageId::Car)), (CarScreen::LogProfile, 2));
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "Boost");
        let ch = find(&v, "Channels");
        p.press(PageId::Car, &v, &Hit::Row(ch), ch as i32, &client);
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "Boost channels");
        let add = find(&v, "Add a channel");
        p.press(PageId::Car, &v, &Hit::Row(add), 0, &client);
        assert_eq!(p.depth(PageId::Car), 4);
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.title, "New channel");
        p.press(PageId::Car, &v, &Hit::Row(find(&v, "Driving")), 0, &client);
        assert_eq!(p.depth(PageId::Car), 5);
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
        assert_eq!(p.depth(PageId::Car), 3);
        let v = p.view(PageId::Car, &s, now);
        assert_eq!(v.rows[3].label, "Speed");
        assert_eq!(v.hit(v.start), Some(Hit::Row(3)));
        assert_eq!(p.recent(), ["vehicle.speed_kmh"]);
        // Sent to carpid (offline here).
        assert_eq!(
            client.snapshot().datalog_note.as_deref(),
            Some("not connected")
        );

        // Back out: channels, profile, logging, car.
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(ch as i32)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(boost as i32)));
        assert_eq!(p.back(PageId::Car), Some(Moved::Closed(4)));
        assert_eq!(p.back(PageId::Car), None);
        p.open(PageId::Car, "logging");
        assert_eq!(p.car, CarScreen::Logging);
    }

    #[test]
    fn the_cluster_settings_edit_in_place() {
        let mut s = snap(&[]);
        s.settings = Some(cfg::sample());
        let p = Pages::new(Saved::default(), None);
        let v = car(CarScreen::Settings, &s, Instant::now(), "", false, &p);
        let patch_of = |act: Option<&Act>| match act {
            Some(Act::Patch(p, false)) => p.dis.clone().unwrap(),
            other => panic!("{other:?}"),
        };
        let claim = find(&v, "Show the page");
        assert_eq!(v.rows[claim].value, "Always");
        assert_eq!(
            patch_of(v.side_act(Some(&Hit::Row(claim)), 1)).claim,
            Some(DisClaim::Tab)
        );
        let page = find(&v, "Default page");
        assert_eq!(v.rows[page].kind, RowKind::Choice);
        assert_eq!(
            patch_of(v.side_act(Some(&Hit::Row(page)), -1)).default_page,
            Some(DisPage::Media)
        );
        let calls = &v.rows[find(&v, "Take over on a call")];
        assert!(calls.checked && calls.kind == RowKind::Toggle);
        assert_eq!(patch_of(Some(&calls.act)).claim_phone, Some(false));
        assert_eq!(v.rows[find(&v, "Media layout")].value, "Text");
        let units = find(&v, "Units");
        assert_eq!(v.rows[units].value, "As the phone");
        assert_eq!(
            patch_of(v.side_act(Some(&Hit::Row(units)), -1)).units,
            Some(DisUnits::Imperial)
        );
        let clock = find(&v, "ETA clock");
        assert_eq!(v.rows[clock].value, "Auto");
        assert_eq!(
            patch_of(v.side_act(Some(&Hit::Row(clock)), 1)).clock,
            Some(DisClock::H12)
        );
        let flash = find(&v, "Cover flash");
        assert_eq!(v.rows[flash].value, "4 s");
        assert_eq!(
            patch_of(v.side_act(Some(&Hit::Row(flash)), -1)).cover_flash_s,
            Some(3.0)
        );
        // A press steps on too (it did nothing as a Step row).
        assert_eq!(
            patch_of(Some(&v.rows[flash].act)).cover_flash_s,
            Some(5.0)
        );
    }

    #[test]
    fn capture_asks_carpid_and_toasts() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let v = p.view(PageId::Car, &snap(&[]), Instant::now());
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
