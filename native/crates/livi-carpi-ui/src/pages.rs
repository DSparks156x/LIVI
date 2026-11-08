// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The three pages: what each shows, and what pressing things does.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use carpi_proto::{
    BusInfo, CurveMode, DarkModeSource, LightAction, LightPatch, PinChange, Settings,
    SettingsPatch, Value,
};

use crate::PageId;
use crate::client::{Capture, Client, Snapshot};
use crate::config::{self, Saved};
use crate::defs::{DEFS, GROUPS, MAX_CELLS, section};
use crate::diag::GROUP_SLOTS;
use crate::light::{self, STEP_AMBIENT, STEP_PCT};
use crate::settings::{self as cfg, DIS_SLOTS};
use crate::values::{NONE, Store, age, show};
use crate::view::{Act, CarScreen, Cell, Hit, Layout, Row, RowKind, View};

/// The light model's curve, JSON for the plot: no use as a reading.
const CURVE: &str = "light.curve";

/// Where the bridge's sections land in carpid's store.
const BRIDGE: [&str; 6] = ["projection.", "nav.", "media.", "call.", "phone.", "ui."];

const SECTIONS: [&str; 10] = [
    "Engine",
    "Chassis",
    "Body",
    "Lights",
    "Light model",
    "Electrical",
    "RNS-E",
    "Haldex",
    "Other",
    "",
];

/// Longest portal PIN the entry takes.
const PIN_MAX: usize = 8;

/// After pressing something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    Stay,
    /// A screen opened: focus its first row.
    Opened,
    /// Went back: focus this.
    Closed(i32),
}

pub struct Pages {
    saved: Saved,
    file: Option<PathBuf>,
    editing: bool,
    module: Option<usize>,
    car: CarScreen,
    trail: Vec<i32>,
    /// The portal PIN being entered.
    pin: String,
    /// The light page shows the manual curve's points.
    advanced: bool,
}

impl Pages {
    /// With the selection from `~/.config/LIVI/carpi-ui.json`.
    pub fn load() -> Self {
        let file = config::path();
        let saved = file.as_deref().map(config::load).unwrap_or_default();
        Self::new(saved, file)
    }

    /// `file` is where changes go; `None` keeps them in memory.
    pub fn new(saved: Saved, file: Option<PathBuf>) -> Self {
        Self {
            saved,
            file,
            editing: false,
            module: None,
            car: CarScreen::Root,
            trail: Vec::new(),
            pin: String::new(),
            advanced: false,
        }
    }

    pub fn saved(&self) -> &Saved {
        &self.saved
    }

    pub fn depth(&self, page: PageId) -> usize {
        match page {
            PageId::Data => usize::from(self.editing),
            PageId::Diagnostics => usize::from(self.module.is_some()),
            PageId::Car => self.car.depth(),
        }
    }

    /// One screen up.
    fn close(&mut self, page: PageId) {
        match page {
            PageId::Data => self.editing = false,
            PageId::Diagnostics => self.module = None,
            PageId::Car => self.car = self.car.parent().unwrap_or(CarScreen::Root),
        }
        if self.car != CarScreen::Pin {
            self.pin.clear();
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

    /// The page's first screen, or the screen `rest` names below it (`light` on the Car page).
    pub fn open(&mut self, page: PageId, rest: &str) {
        self.reset(page);
        if page == PageId::Car && rest.split('/').next() == Some("light") {
            self.car = CarScreen::Light;
            self.trail.push(0);
        }
    }

    /// Back to the page's first screen.
    pub fn reset(&mut self, page: PageId) {
        while self.depth(page) > 0 {
            self.close(page);
        }
        self.trail.clear();
    }

    /// One screen up. `None` on a page's first screen, where back leaves the page.
    pub fn back(&mut self, page: PageId) -> Option<i32> {
        if self.depth(page) == 0 {
            return None;
        }
        self.close(page);
        Some(self.trail.pop().unwrap_or(0))
    }

    /// Presses what `hit` points at on `view`, the view of `page` the user saw. `focus` is where
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
            return self.back(page).map_or(Moved::Stay, Moved::Closed);
        }
        let Some(act) = view.act(hit) else {
            return Moved::Stay;
        };
        let group = self.saved.group();
        let opened = match act {
            Act::None => false,
            Act::Group(id) => {
                self.saved.group = id.to_string();
                self.store();
                false
            }
            Act::Edit => {
                self.editing = true;
                true
            }
            Act::Reset => {
                self.saved.reset(group);
                self.store();
                false
            }
            Act::Toggle(name) => {
                if self.saved.toggle(group, name) {
                    self.store();
                }
                false
            }
            Act::Module(i) => {
                self.module = Some(*i);
                true
            }
            Act::Car(screen) => {
                self.car = *screen;
                *screen != CarScreen::Root
            }
            Act::Capture => {
                client.save_capture();
                false
            }
            Act::Patch(patch, back) => {
                client.set((**patch).clone());
                if *back {
                    return self.back(page).map_or(Moved::Stay, Moved::Closed);
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
            Act::Light(action) => {
                client.light(*action);
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
                return self.back(page).map_or(Moved::Stay, Moved::Closed);
            }
        };
        if opened {
            self.trail.push(focus);
            Moved::Opened
        } else {
            Moved::Stay
        }
    }

    fn store(&self) {
        let Some(file) = &self.file else { return };
        if let Err(e) = config::save(file, &self.saved) {
            eprintln!("[carpi-ui] can't save {}: {e}", file.display());
        }
    }

    pub fn view(&self, page: PageId, snap: &Snapshot, now: Instant) -> View {
        match page {
            PageId::Data if self.editing => self.edit(snap, now),
            PageId::Data => self.grid(snap, now),
            PageId::Diagnostics => match self.module {
                Some(i) => module(snap, i),
                None => modules(snap),
            },
            PageId::Car => car(self.car, snap, now, &self.pin, self.advanced),
        }
    }

    fn grid(&self, snap: &Snapshot, now: Instant) -> View {
        let group = self.saved.group();
        let names = self.saved.values(group);
        let note = if !snap.connected {
            problem(snap)
        } else if names.is_empty() {
            "Nothing picked. Edit adds values.".into()
        } else {
            String::new()
        };
        let mut v = View::new(Layout::Grid, "", false).note(note);
        for g in GROUPS {
            v.pill(g.label, g.id == group, true, Act::Group(g.id));
        }
        v.pill("Edit", false, true, Act::Edit);
        v.cells = names
            .iter()
            .map(|n| {
                let s = show(n, snap.values.get(n), now);
                Cell {
                    dim: s.freshness.dim(),
                    label: s.label,
                    name: s.name,
                    value: s.value,
                    unit: s.unit,
                    age: s.age,
                }
            })
            .collect();
        v.finish()
    }

    fn edit(&self, snap: &Snapshot, now: Instant) -> View {
        let group = self.saved.group();
        let label = GROUPS
            .iter()
            .find(|g| g.id == group)
            .map_or("", |g| g.label);
        let picked = self.saved.values(group);
        let note = format!(
            "{} of {MAX_CELLS} picked. Kept in ~/.config/LIVI/carpi-ui.json until carpid keeps page settings.",
            picked.len()
        );
        let mut v = View::new(Layout::List, format!("{label} values"), true).note(note);
        v.pill(
            "Reset",
            false,
            self.saved.groups.contains_key(group),
            Act::Reset,
        );
        let mut at = "";
        for name in candidates(&snap.values) {
            let sec = section(&name);
            if sec != at {
                v.section(sec);
                at = sec;
            }
            let s = show(&name, snap.values.get(&name), now);
            let checked = picked.contains(&name);
            let mut row = Row::new(
                RowKind::Toggle,
                format!("{} ({name})", s.label),
                reading(&s.value, &s.unit),
            )
            .enabled(checked || picked.len() < MAX_CELLS)
            .act(Act::Toggle(name.clone()));
            row.checked = checked;
            v.row(row);
        }
        v.finish()
    }
}

fn problem(snap: &Snapshot) -> String {
    snap.problem
        .clone()
        .unwrap_or_else(|| "carpid not connected".into())
}

fn reading(value: &str, unit: &str) -> String {
    if unit.is_empty() || value == NONE {
        value.to_string()
    } else {
        format!("{value} {unit}")
    }
}

/// Every value the picker offers: the known ones, then whatever else carpid has, by section.
fn candidates(store: &Store) -> Vec<String> {
    let mut names: Vec<String> = DEFS.iter().map(|d| d.name.to_string()).collect();
    let known = names.len();
    names.extend(
        store
            .names()
            .filter(|n| !BRIDGE.iter().any(|p| n.starts_with(p)) && *n != CURVE)
            .filter(|n| !DEFS.iter().any(|d| d.name == *n))
            .map(str::to_string),
    );
    let rank = |n: &str| {
        SECTIONS
            .iter()
            .position(|s| *s == section(n))
            .unwrap_or(SECTIONS.len())
    };
    let mut ordered: Vec<(usize, usize, String)> = names
        .into_iter()
        .enumerate()
        .map(|(i, n)| (rank(&n), if i < known { i } else { known }, n))
        .collect();
    ordered.sort();
    ordered.into_iter().map(|(_, _, n)| n).collect()
}

/// A value as an info row: "Label (name)" and its reading, dimmed when not fresh.
fn value_row(name: &str, snap: &Snapshot, now: Instant) -> Row {
    let s = show(name, snap.values.get(name), now);
    let value = if s.freshness.dim() && s.value != NONE {
        format!("{} · {}", reading(&s.value, &s.unit), s.age)
    } else {
        reading(&s.value, &s.unit)
    };
    Row::new(RowKind::Info, format!("{} ({name})", s.label), value).dimmed(s.freshness.dim())
}

/// Known names under `prefix` in table order, then any others carpid has.
fn names_under(prefix: &str, store: &Store) -> Vec<String> {
    let mut names: Vec<String> = DEFS
        .iter()
        .filter(|d| d.name.starts_with(prefix))
        .map(|d| d.name.to_string())
        .collect();
    for (n, _) in store.under(prefix) {
        if !names.iter().any(|m| m == n) {
            names.push(n.to_string());
        }
    }
    names
}

fn modules(snap: &Snapshot) -> View {
    let diag = &snap.diag;
    let note = if diag.available() {
        String::new()
    } else {
        "Not available yet: carpid has no diagnostics API. Default module list.".into()
    };
    let mut v = View::new(Layout::List, "Diagnostics", false).note(note);
    v.pill("Scan", false, false, Act::None);
    v.section(if diag.available() {
        "Install list"
    } else {
        "Modules"
    });
    for (i, m) in diag.modules().iter().enumerate() {
        let value = match m.faults {
            Some(0) => "OK".to_string(),
            Some(n) => format!("{n} faults"),
            None => NONE.to_string(),
        };
        let mut row = Row::new(
            RowKind::Route,
            format!("{:02X} {}", m.module.address, m.module.name),
            value,
        )
        .act(Act::Module(i));
        row.alert = m.faults.is_some_and(|n| n > 0);
        v.row(row);
    }
    v.finish()
}

fn module(snap: &Snapshot, i: usize) -> View {
    let diag = &snap.diag;
    let list = diag.modules();
    let Some(m) = list.get(i) else {
        return View::new(Layout::List, "Diagnostics", true).finish();
    };
    let on = diag.available();
    let not_yet = || "not available yet".to_string();
    let addr = m.module.address;
    let note = if on {
        String::new()
    } else {
        "Not available yet: carpid has no diagnostics API.".into()
    };
    let mut v = View::new(Layout::List, format!("{addr:02X} {}", m.module.name), true).note(note);
    v.section("Identification");
    let or_none = |s: &Option<String>| s.clone().unwrap_or_else(|| NONE.into());
    v.row(Row::new(RowKind::Info, "Part number", or_none(&m.part)));
    v.row(Row::new(RowKind::Info, "Component", or_none(&m.component)));
    v.row(Row::new(RowKind::Info, "Coding", or_none(&m.coding)));
    let mut tp2 = Row::new(RowKind::Info, "TP2 ID", format!("0x{:02X}", m.module.tp2));
    tp2.mono = true;
    v.row(tp2);
    v.section("Fault codes");
    match diag.dtcs.get(&addr) {
        Some(codes) if codes.is_empty() => v.row(Row::new(RowKind::Info, "No faults", "")),
        Some(codes) => {
            for c in codes {
                let mut row = Row::new(RowKind::Info, format!("{} {}", c.code, c.text), &c.status);
                row.alert = true;
                v.row(row);
            }
        }
        None => v.row(Row::new(RowKind::Info, "Fault codes", not_yet())),
    }
    let mut read = Row::new(RowKind::Button, "Read fault codes", "").enabled(false);
    read.button = "Read".into();
    v.row(read);
    let mut clear = Row::new(RowKind::Button, "Clear fault codes", "").enabled(false);
    clear.button = "Clear".into();
    v.row(clear);
    v.section("Measuring groups");
    let shown: Vec<(&(u8, u8), _)> = diag.groups.range((addr, 0)..=(addr, u8::MAX)).collect();
    for slot in 0..GROUP_SLOTS {
        let row = match shown.get(slot) {
            Some(((_, g), fields)) => Row::new(
                RowKind::Info,
                format!("Group {g:03}"),
                fields
                    .iter()
                    .map(|f| reading(&f.value, &f.unit))
                    .collect::<Vec<_>>()
                    .join("  ·  "),
            ),
            None => Row::new(RowKind::Info, format!("Group slot {}", slot + 1), not_yet()),
        };
        v.row(row.enabled(on));
    }
    v.finish()
}

fn car(screen: CarScreen, snap: &Snapshot, now: Instant, pin: &str, advanced: bool) -> View {
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
        CarScreen::Reading(i) => dis_slot(snap, now, i),
        CarScreen::Bindings => bindings(snap),
        CarScreen::Binding(i) => binding(snap, i),
        CarScreen::Pin => pin_entry(snap, pin),
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
        _ => NONE.into(),
    };
    let light = match flag(snap, "light.night") {
        Some(true) => "Night",
        Some(false) => "Day",
        None => NONE,
    };
    let rnse = match (flag(snap, "rnse.on_tv"), flag(snap, "rnse.radio_on")) {
        (Some(true), _) => "On TV",
        (_, Some(true)) => "On",
        (_, Some(false)) => "Off",
        _ => NONE,
    };
    let projection = match snap
        .values
        .get("projection.kind")
        .and_then(|s| s.value.as_ref())
    {
        Some(Value::Text(k)) if k == "android_auto" => "Android Auto",
        Some(Value::Text(k)) if k == "carplay" => "CarPlay",
        _ => "None",
    };
    v.section("State");
    for (label, value, screen) in [
        ("Buses", buses.as_str(), CarScreen::Buses),
        ("Light", light, CarScreen::Light),
        ("RNS-E", rnse, CarScreen::Rnse),
        ("Projection and bridge", projection, CarScreen::Bridge),
    ] {
        v.row(Row::new(RowKind::Route, label, value).act(Act::Car(screen)));
    }
    v.section("Capture");
    let mut save = Row::new(RowKind::Button, "CAN capture", "")
        .enabled(snap.connected && snap.capture != Capture::Saving)
        .act(Act::Capture);
    save.button = "Save capture".into();
    v.row(save);
    let (last, alert) = match &snap.capture {
        Capture::Idle => (NONE.to_string(), false),
        Capture::Saving => ("saving…".to_string(), false),
        Capture::Saved { path, frames } => (format!("{path} ({frames} frames)"), false),
        Capture::Failed(why) => (format!("failed: {why}"), true),
    };
    let mut row = Row::new(RowKind::Info, "Last capture", last);
    row.alert = alert;
    row.mono = matches!(snap.capture, Capture::Saved { .. });
    v.row(row);
    v.section("Settings");
    let tx = match &snap.settings {
        Some(s) if s.tx_enabled => "TX on",
        Some(_) => "TX off",
        None => NONE,
    };
    v.row(Row::new(RowKind::Route, "Settings", tx).act(Act::Car(CarScreen::Settings)));
    v.finish()
}

fn bus_rows(v: &mut View, buses: Option<&[BusInfo]>) {
    let Some(buses) = buses.filter(|b| !b.is_empty()) else {
        v.row(Row::new(RowKind::Info, "No bus data", NONE));
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
                format!("Node 0x{:02X}", m.node),
                format!(
                    "next 0x{:02X} · {} · {}",
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
    let mut v = View::new(Layout::List, "Projection and bridge", true).note(note);
    v.section("carpid");
    v.row(Row::new(
        RowKind::Info,
        "Daemon",
        snap.daemon.clone().unwrap_or_else(|| NONE.into()),
    ));
    let link = if snap.connected {
        "connected".to_string()
    } else {
        problem(snap)
    };
    v.row(Row::new(RowKind::Info, "Pages link", link));
    let mut any = false;
    for p in BRIDGE {
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
        v.section(format!("Bridge: {}", p.trim_end_matches('.')));
        for n in names {
            v.row(value_row(&n, snap, now));
        }
    }
    if !any {
        v.section("Bridge");
        v.row(Row::new(
            RowKind::Info,
            "Nothing pushed",
            "is livi-carpi connected?",
        ));
    }
    v.finish()
}

/// The note over a settings screen: no connection, a failed change, or a restart to do.
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
    let mut row = Row::new(RowKind::Toggle, label, "").act(act);
    row.checked = on;
    row
}

/// A settings screen, or one saying they haven't come yet.
fn with_settings(snap: &Snapshot, title: &str, body: impl FnOnce(&mut View, &Settings)) -> View {
    let mut v = View::new(Layout::List, title, true).note(settings_note(snap));
    match &snap.settings {
        Some(s) => body(&mut v, s),
        None => v.row(Row::new(RowKind::Info, "Settings", "not from carpid yet").dimmed(true)),
    }
    v.finish()
}

fn settings(snap: &Snapshot) -> View {
    with_settings(snap, "Settings", |v, s| {
        v.section("CAN");
        v.row(toggle(
            "TX enabled",
            s.tx_enabled,
            patch(
                SettingsPatch {
                    tx_enabled: Some(!s.tx_enabled),
                    ..Default::default()
                },
                false,
            ),
        ));
        v.section("Power");
        v.row(toggle(
            "Shutdown on request",
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
        v.section("Keys");
        v.row(
            Row::new(
                RowKind::Route,
                "Key bindings",
                format!("{} rows", s.bindings.len()),
            )
            .act(Act::Car(CarScreen::Bindings)),
        );
        v.section("Phone portal");
        let set = if s.portal_pin_set { "set" } else { "not set" };
        v.row(Row::new(RowKind::Route, "PIN", set).act(Act::Car(CarScreen::Pin)));
        let mut clear = Row::new(RowKind::Button, "Clear the PIN", "")
            .enabled(s.portal_pin_set)
            .act(patch(
                SettingsPatch {
                    portal_pin: Some(PinChange::Clear),
                    ..Default::default()
                },
                false,
            ));
        clear.button = "Clear".into();
        v.row(clear);
    })
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

fn button(label: &str, value: impl Into<String>, text: &str, act: Act) -> Row {
    let mut row = Row::new(RowKind::Button, label, value).act(act);
    row.button = text.into();
    row
}

/// PLAN.md 2.6: the plot and the nudge first, the options after, manual editing behind a switch.
fn light_page(snap: &Snapshot, advanced: bool) -> View {
    let settings = snap.settings.as_ref().map(|s| &s.light);
    let ambient = num(snap, "light.ambient");
    let pct = num(snap, "light.panel_pct");
    let curve = text_of(snap, CURVE).and_then(light::parse);
    let auto = settings.is_none_or(|l| l.auto_brightness);
    let mut v = View::new(Layout::List, "Light", true).note(settings_note(snap));
    let nudge = |delta_pct| Act::Light(LightAction::Nudge { delta_pct });
    let can = snap.connected && auto;
    v.pill("Darker", false, can, nudge(-(STEP_PCT as i8)));
    v.pill("Brighter", false, can, nudge(STEP_PCT as i8));
    v.plot = Some(light::plot(
        curve.as_ref(),
        settings,
        ambient,
        pct,
        advanced,
    ));

    v.section("Now");
    v.row(Row::new(
        RowKind::Info,
        "Ambient",
        ambient.map_or_else(|| NONE.to_string(), ambient_text),
    ));
    let panel = match pct {
        Some(p) if auto => format!("{p:.0} %"),
        Some(p) => format!("{p:.0} %, the car's dimming"),
        None => NONE.to_string(),
    };
    v.row(Row::new(RowKind::Info, "RNS-E panel", panel));
    let night = match flag(snap, "light.night") {
        Some(true) => "Night",
        Some(false) => "Day",
        None => NONE,
    };
    let dark = match text_of(snap, "light.night_source") {
        Some(by) => format!("{night}, by the {by}"),
        None => night.to_string(),
    };
    v.row(Row::new(RowKind::Info, "Dark mode", dark));

    let Some(l) = settings else {
        v.row(Row::new(RowKind::Info, "Settings", "not from carpid yet").dimmed(true));
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
    v.row(button(
        "Ambient sensor",
        light::source_name(l.ambient_source),
        "Change",
        light_patch(
            LightPatch {
                ambient_source: Some(light::next_source(l.ambient_source)),
                ..Default::default()
            },
            false,
        ),
    ));
    let other = match l.curve_mode {
        CurveMode::Manual => CurveMode::Learned,
        CurveMode::Learned => CurveMode::Manual,
    };
    v.row(button(
        "Curve",
        light::mode_name(l.curve_mode),
        "Change",
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
    v.row(
        button(
            "Learned points",
            format!("{learned} for this sensor"),
            "Reset",
            Act::Light(LightAction::ResetLearned { source: None }),
        )
        .enabled(snap.connected && learned > 0),
    );
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
        let here = ambient.unwrap_or(0.0);
        let level = pct.map_or(50, |p| p.round().clamp(0.0, 100.0) as u8);
        v.row(
            button(
                "Add a point at the current ambient",
                format!("{level} %"),
                "Add",
                light_patch(
                    LightPatch {
                        manual_curve: Some(light::added(&l.manual_curve, here, level)),
                        ..Default::default()
                    },
                    false,
                ),
            )
            .enabled(ambient.is_some()),
        );
    }

    v.section("Dark mode");
    let by = match l.dark_mode_source {
        DarkModeSource::Headlights => "headlights; the clock with the ignition off",
        DarkModeSource::Ambient => "ambient; the clock while it's stale",
    };
    v.row(Row::new(RowKind::Info, "Decided by", by));
    v.row(Row::new(
        RowKind::Info,
        "Night threshold",
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
    v.row(Row::new(RowKind::Info, "Clock", clock));
    v.section("Values");
    v.row(Row::new(RowKind::Route, "All light values", "").act(Act::Car(CarScreen::LightValues)));
    v.finish()
}

fn curve_point(snap: &Snapshot, at: usize) -> View {
    with_settings(snap, &format!("Curve point {}", at + 1), |v, s| {
        let c = &s.light.manual_curve;
        let Some(p) = c.get(at) else {
            v.row(Row::new(RowKind::Info, "No such point", NONE));
            return;
        };
        v.row(Row::new(RowKind::Info, "Ambient", ambient_text(p.ambient)));
        v.row(Row::new(
            RowKind::Info,
            "RNS-E panel",
            format!("{} %", p.pct),
        ));
        let edit = |pct: i16, ambient: f64| {
            light_patch(
                LightPatch {
                    manual_curve: Some(light::moved(c, at, pct, ambient)),
                    ..Default::default()
                },
                false,
            )
        };
        v.section("Change");
        v.row(button("Panel", "brighter", "+5 %", edit(STEP_PCT, 0.0)));
        v.row(button("Panel", "darker", "−5 %", edit(-STEP_PCT, 0.0)));
        v.row(button("Ambient", "brighter", "+0.1", edit(0, STEP_AMBIENT)));
        v.row(button("Ambient", "darker", "−0.1", edit(0, -STEP_AMBIENT)));
        v.row(
            button(
                "Remove this point",
                "",
                "Remove",
                light_patch(
                    LightPatch {
                        manual_curve: Some(light::removed(c, at)),
                        ..Default::default()
                    },
                    true,
                ),
            )
            .enabled(c.len() > 2),
        );
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
            let value = list.get(i).map_or_else(
                || "empty".to_string(),
                |r| format!("{} · {}", r.label, r.key),
            );
            v.row(
                Row::new(RowKind::Route, slot_name(i), value).act(Act::Car(CarScreen::Reading(i))),
            );
        }
    })
}

fn dis_slot(snap: &Snapshot, now: Instant, at: usize) -> View {
    with_settings(snap, &slot_name(at), |v, s| {
        let list = &s.dis.readings;
        let current = list.get(at).map(|r| r.key.as_str());
        let set = |name: Option<&str>| {
            patch(
                SettingsPatch {
                    dis_readings: Some(cfg::reading_set(list, at, name)),
                    ..Default::default()
                },
                true,
            )
        };
        v.row(toggle("Nothing", current.is_none(), set(None)).enabled(at < list.len()));
        let mut section_at = "";
        for name in candidates(&snap.values) {
            let sec = section(&name);
            if sec != section_at {
                v.section(sec);
                section_at = sec;
            }
            let shown = show(&name, snap.values.get(&name), now);
            let mut row = toggle(
                &format!("{} ({name})", shown.label),
                current == Some(name.as_str()),
                set(Some(&name)),
            );
            row.value = reading(&shown.value, &shown.unit);
            v.row(row);
        }
    })
}

fn bindings(snap: &Snapshot) -> View {
    with_settings(snap, "Key bindings", |v, s| {
        let mut table = "";
        for (i, b) in s.bindings.iter().enumerate() {
            if b.table != table {
                v.section(b.table.clone());
                table = &b.table;
            }
            let mut row = Row::new(
                RowKind::Route,
                format!("{} {}", b.key, b.gesture),
                b.action.clone(),
            )
            .act(Act::Car(CarScreen::Binding(i)));
            row.mono = true;
            v.row(row);
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
            v.row(Row::new(RowKind::Info, "No such row", NONE));
            return;
        };
        let mut actions: Vec<&str> = cfg::actions(&b.table).to_vec();
        if !actions.contains(&b.action.as_str()) {
            actions.insert(0, &b.action);
        }
        v.section("Action");
        for a in actions {
            let set = SettingsPatch {
                bindings: Some(cfg::rebound(&s.bindings, at, a)),
                ..Default::default()
            };
            v.row(toggle(a, a == b.action, patch(set, true)));
        }
    })
}

fn pin_entry(snap: &Snapshot, pin: &str) -> View {
    let mut v = View::new(Layout::List, "Portal PIN", true).note(settings_note(snap));
    v.pill("Delete", false, !pin.is_empty(), Act::PinDelete);
    v.pill(
        "Save",
        true,
        snap.connected && !pin.is_empty(),
        Act::PinSave,
    );
    let shown = if pin.is_empty() {
        NONE.to_string()
    } else {
        "•".repeat(pin.len())
    };
    v.section("New PIN");
    v.row(Row::new(RowKind::Info, "PIN", shown));
    v.section("Digits");
    for d in (1..=9).chain([0]) {
        let mut row = Row::new(RowKind::Button, d.to_string(), "")
            .enabled(pin.len() < PIN_MAX)
            .act(Act::PinDigit(d));
        row.button = "Add".into();
        v.row(row);
    }
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

    #[test]
    fn the_grid_shows_the_group_and_greys_invalid_values() {
        let p = Pages::new(Saved::default(), None);
        let s = snap(&[
            ("engine.rpm", Some(Value::Num(900.0))),
            ("engine.coolant_c", None),
        ]);
        let v = p.view(PageId::Data, &s, Instant::now());
        assert_eq!(v.layout, Layout::Grid);
        assert_eq!(v.pills.len(), GROUPS.len() + 1);
        assert!(v.pills[0].active);
        assert_eq!(v.cells[0].value, "900");
        assert!(!v.cells[0].dim);
        let coolant = v
            .cells
            .iter()
            .find(|c| c.name == "engine.coolant_c")
            .unwrap();
        assert!(coolant.dim);
        assert_eq!(coolant.value, NONE);
    }

    #[test]
    fn edit_opens_toggles_and_comes_back() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[]);
        let v = p.view(PageId::Data, &s, Instant::now());
        let edit = Hit::Pill(GROUPS.len());
        assert_eq!(p.press(PageId::Data, &v, &edit, 4, &client), Moved::Opened);
        assert_eq!(p.depth(PageId::Data), 1);
        let v = p.view(PageId::Data, &s, Instant::now());
        assert_eq!(v.back_at, 0);
        let rpm = v
            .rows
            .iter()
            .position(|r| r.act == Act::Toggle("engine.rpm".into()))
            .unwrap();
        assert!(v.rows[rpm].checked);
        p.press(PageId::Data, &v, &Hit::Row(rpm), 9, &client);
        assert!(!p.saved().values("engine").contains(&"engine.rpm".into()));
        assert_eq!(
            p.press(PageId::Data, &v, &Hit::Back, 0, &client),
            Moved::Closed(4)
        );
        assert_eq!(p.depth(PageId::Data), 0);
        assert_eq!(p.back(PageId::Data), None);
    }

    #[test]
    fn diagnostics_say_not_available() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[]);
        let v = p.view(PageId::Diagnostics, &s, Instant::now());
        assert!(v.note.contains("not available") || v.note.contains("Not available"));
        assert!(!v.pills[0].enabled);
        assert_eq!(v.rows[1].label, "01 Engine");
        assert_eq!(
            p.press(PageId::Diagnostics, &v, &Hit::Row(1), 1, &client),
            Moved::Opened
        );
        let v = p.view(PageId::Diagnostics, &s, Instant::now());
        assert_eq!(v.title, "01 Engine");
        assert!(v.rows.iter().any(|r| r.value == "not available yet"));
        assert!(
            v.rows
                .iter()
                .filter(|r| r.kind == RowKind::Button)
                .all(|r| !r.enabled)
        );
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
        let now = Instant::now();
        let v = car(CarScreen::Root, &s, now, "", false);
        let values: Vec<&str> = v.rows.iter().map(|r| r.value.as_str()).collect();
        assert!(values.contains(&"ican awake"));
        assert!(values.contains(&"Night"));
        assert!(values.contains(&"On TV"));
        assert!(values.contains(&"Android Auto"));
        let v = car(CarScreen::Buses, &s, now, "", false);
        assert!(v.rows.iter().any(|r| r.value == "1234"));
        assert!(v.rows.iter().any(|r| r.label == "Node 0x20"));
        let v = car(CarScreen::Bridge, &s, now, "", false);
        assert!(v.rows.iter().any(|r| r.label.contains("nav.road")));
        assert!(!v.rows.iter().any(|r| r.label.contains("base64")));
        let v = car(CarScreen::Settings, &s, now, "", false);
        assert!(v.rows.iter().any(|r| r.value == "not from carpid yet"));
    }

    #[test]
    fn settings_edit_through_carpid() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap(&[("engine.rpm", Some(Value::Num(800.0)))]);
        s.settings = Some(cfg::sample());
        let now = Instant::now();
        let v = car(CarScreen::Settings, &s, now, "", false);
        let tx = v.rows.iter().find(|r| r.label == "TX enabled").unwrap();
        assert!(!tx.checked);
        let Act::Patch(patch, false) = &tx.act else {
            panic!("{:?}", tx.act)
        };
        assert_eq!(patch.tx_enabled, Some(true));
        let clear = v.rows.iter().find(|r| r.button == "Clear").unwrap();
        assert!(!clear.enabled);

        // Settings, then bindings, then one row: three deep, and back twice after picking.
        let v = p.view(PageId::Car, &s, now);
        let at = v
            .rows
            .iter()
            .position(|r| r.label == "Settings" && r.kind == RowKind::Route)
            .unwrap();
        p.press(PageId::Car, &v, &Hit::Row(at), 3, &client);
        let v = p.view(PageId::Car, &s, now);
        let at = v
            .rows
            .iter()
            .position(|r| r.label == "Key bindings")
            .unwrap();
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
        let select = v.rows.iter().position(|r| r.label == "select").unwrap();
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
    fn the_light_page_plots_nudges_and_edits_the_curve() {
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
        assert!(v.rows.iter().any(|r| r.value == "Day, by the headlights"));
        assert!(v.rows.iter().any(|r| r.value == "not from carpid yet"));
        assert_eq!(
            v.pills[1].act,
            Act::Light(LightAction::Nudge { delta_pct: 5 })
        );
        p.press(PageId::Car, &v, &Hit::Pill(1), 1, &client);
        assert_eq!(
            client.snapshot().settings_note.as_deref(),
            Some("not connected")
        );

        s.settings = Some(cfg::sample());
        let v = p.view(PageId::Car, &s, now);
        let trim = v.rows.iter().find(|r| r.label == "Scroll trim").unwrap();
        assert!(trim.checked);
        assert!(v.rows.iter().any(|r| r.label == "Night threshold"));
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
        let up = v.rows.iter().find(|r| r.button == "+5 %").unwrap();
        let Act::Patch(patch, false) = &up.act else {
            panic!("{:?}", up.act)
        };
        let points = patch.light.as_ref().unwrap().manual_curve.as_ref().unwrap();
        assert_eq!(points[1].pct, 55);
        assert_eq!(p.back(PageId::Car), Some(4));
        assert_eq!(p.back(PageId::Car), Some(0));
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
        assert!(!v.pills[1].enabled);
        for d in [4, 7, 1, 1] {
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
    fn capture_asks_carpid() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap(&[]);
        let v = p.view(PageId::Car, &s, Instant::now());
        let at = v.rows.iter().position(|r| r.act == Act::Capture).unwrap();
        assert_eq!(
            p.press(PageId::Car, &v, &Hit::Row(at), 0, &client),
            Moved::Stay
        );
        assert!(matches!(client.snapshot().capture, Capture::Failed(_)));
    }

    #[test]
    fn the_picker_lists_known_values_then_carpids_own() {
        let s = snap(&[("engine.new_thing_c", Some(Value::Num(1.0)))]);
        let names = candidates(&s.values);
        assert_eq!(names[0], "engine.rpm");
        let new = names
            .iter()
            .position(|n| n == "engine.new_thing_c")
            .unwrap();
        let next = &names[new + 1];
        assert_ne!(section(next), "Engine");
        assert!(!names.iter().any(|n| n.starts_with("nav.")));
    }
}
