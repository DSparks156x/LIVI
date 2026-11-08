// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The three pages: what each shows, and what pressing things does.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use carpi_proto::{BusInfo, Value};

use crate::PageId;
use crate::client::{Capture, Client, Snapshot};
use crate::config::{self, Saved};
use crate::defs::{DEFS, GROUPS, MAX_CELLS, section};
use crate::diag::GROUP_SLOTS;
use crate::values::{NONE, Store, age, show};
use crate::view::{Act, CarScreen, Cell, Hit, Layout, Row, RowKind, View};

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
        }
    }

    pub fn saved(&self) -> &Saved {
        &self.saved
    }

    pub fn depth(&self, page: PageId) -> usize {
        let deep = match page {
            PageId::Data => self.editing,
            PageId::Diagnostics => self.module.is_some(),
            PageId::Car => self.car != CarScreen::Root,
        };
        usize::from(deep)
    }

    fn close(&mut self, page: PageId) {
        match page {
            PageId::Data => self.editing = false,
            PageId::Diagnostics => self.module = None,
            PageId::Car => self.car = CarScreen::Root,
        }
    }

    /// Back to the page's first screen.
    pub fn reset(&mut self, page: PageId) {
        self.close(page);
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
            PageId::Car => car(self.car, snap, now),
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
            .filter(|n| !BRIDGE.iter().any(|p| n.starts_with(p)))
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

fn car(screen: CarScreen, snap: &Snapshot, now: Instant) -> View {
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
        CarScreen::Light => listed(snap, now, note, "Light", &["light."]),
        CarScreen::Rnse => listed(snap, now, note, "RNS-E", &["rnse."]),
        CarScreen::Bridge => bridge(snap, now, note),
        CarScreen::Settings => settings(),
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
    v.row(
        Row::new(RowKind::Route, "Settings", "read-only, API coming")
            .act(Act::Car(CarScreen::Settings)),
    );
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

fn settings() -> View {
    let mut v = View::new(Layout::List, "Settings", true)
        .note("Read-only: settings API coming. carpid's config file still decides.");
    for (section, rows) in [
        (
            "Brightness",
            &[
                "Night threshold",
                "Day threshold",
                "Panel level by day",
                "Panel level at night",
            ][..],
        ),
        ("CAN", &["TX enabled"][..]),
        ("Power", &["Shutdown enabled"][..]),
        ("Keys", &["Key bindings"][..]),
    ] {
        v.section(section);
        for r in rows {
            v.row(Row::new(RowKind::Info, *r, NONE).enabled(false));
        }
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
        let v = car(CarScreen::Root, &s, now);
        let values: Vec<&str> = v.rows.iter().map(|r| r.value.as_str()).collect();
        assert!(values.contains(&"ican awake"));
        assert!(values.contains(&"Night"));
        assert!(values.contains(&"On TV"));
        assert!(values.contains(&"Android Auto"));
        let v = car(CarScreen::Buses, &s, now);
        assert!(v.rows.iter().any(|r| r.value == "1234"));
        assert!(v.rows.iter().any(|r| r.label == "Node 0x20"));
        let v = car(CarScreen::Bridge, &s, now);
        assert!(v.rows.iter().any(|r| r.label.contains("nav.road")));
        assert!(!v.rows.iter().any(|r| r.label.contains("base64")));
        let v = car(CarScreen::Settings, &s, now);
        assert!(v.note.contains("settings API coming"));
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
