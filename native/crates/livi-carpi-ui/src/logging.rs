// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Logging screens under the Car page (docs/PAGES.md 4): start and stop a run, and the
//! profiles carpid keeps. A profile is a name from a fixed list, a rate, and a channel set edited
//! with the slot editor's list form. Every change goes to carpid at once, like the Data page's.

use std::time::Instant;

use carpi_proto::{DATALOG_RATES, DatalogAction, DatalogLoad, DatalogProfile, DatalogStatus};

use crate::client::{Client, Snapshot};
use crate::slots::{self, Editor, Focus, SlotAct};
use crate::view::{Act, CarScreen, Layout, Row, RowKind, View};

/// Profile names, cycled like the Data page's. Others come from the portal.
pub const NAMES: &[&str] = &[
    "Engine",
    "Boost",
    "Haldex",
    "Electrical",
    "Trip",
    "Custom 1",
    "Custom 2",
    "Custom 3",
    "Custom 4",
];

/// A new profile's rate.
const RATE: u32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub enum LogAct {
    Start,
    Stop,
    /// The D-pad on the profile row: the one Start uses.
    Pick(i32),
    /// Opens a profile by name.
    Open(String),
    /// A new profile with the next free name, opened.
    Add,
    Name(i32),
    Rate(i32),
    /// Asks, then removes the open profile.
    Remove,
}

/// What an act did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Stay,
    Opened(CarScreen),
    Back,
}

/// The Logging screens' own state.
#[derive(Debug, Clone, Default)]
pub struct Logging {
    /// The profile Start uses, by name.
    pub pick: Option<String>,
    /// The open profile: its name at carpid, and the copy being edited.
    pub open: Option<(String, DatalogProfile)>,
    /// The channel list editor and its picker.
    pub editor: Editor,
    /// "Remove this profile" waits for a second push.
    pub removing: bool,
}

impl Logging {
    /// The profile Start uses: the one picked here, else carpid's last, else the first.
    pub fn pick<'a>(&self, st: &'a DatalogStatus) -> Option<&'a DatalogProfile> {
        let named = |n: &Option<String>| {
            n.as_ref()
                .and_then(|n| st.profiles.iter().find(|p| p.profile.name == *n))
        };
        named(&self.pick)
            .or_else(|| named(&st.selected))
            .or(st.profiles.first())
            .map(|p| &p.profile)
    }

    /// Back to the first screen; the pick stays.
    pub fn reset(&mut self) {
        self.open = None;
        self.editor = Editor::default();
        self.removing = false;
    }

    pub fn apply(&mut self, a: &LogAct, st: Option<&DatalogStatus>, client: &Client) -> Done {
        if *a != LogAct::Remove {
            self.removing = false;
        }
        let names: Vec<String> = st
            .map(|s| s.profiles.iter().map(|p| p.profile.name.clone()).collect())
            .unwrap_or_default();
        match a {
            LogAct::Start => {
                if let Some(p) = st.and_then(|s| self.pick(s)) {
                    client.datalog(DatalogAction::Start {
                        profile: p.name.clone(),
                    });
                }
                Done::Stay
            }
            LogAct::Stop => {
                client.datalog(DatalogAction::Stop);
                Done::Stay
            }
            LogAct::Pick(d) => {
                let Some(now) = st.and_then(|s| self.pick(s)) else {
                    return Done::Stay;
                };
                let at = names.iter().position(|n| *n == now.name).unwrap_or(0) as i32;
                let n = names.len() as i32;
                self.pick = Some(names[(at + d).rem_euclid(n) as usize].clone());
                Done::Stay
            }
            LogAct::Open(name) => {
                let Some(p) = st.and_then(|s| s.profiles.iter().find(|p| p.profile.name == *name))
                else {
                    return Done::Stay;
                };
                self.open = Some((name.clone(), p.profile.clone()));
                self.editor = Editor::default();
                Done::Opened(CarScreen::LogProfile)
            }
            LogAct::Add => {
                let Some(name) = NAMES.iter().find(|n| !names.iter().any(|x| x == *n)) else {
                    return Done::Stay;
                };
                let p = DatalogProfile {
                    name: name.to_string(),
                    values: Vec::new(),
                    rate_hz: RATE,
                };
                client.datalog(DatalogAction::Save {
                    profile: p.clone(),
                    replaces: None,
                });
                self.open = Some((p.name.clone(), p));
                self.editor = Editor::default();
                Done::Opened(CarScreen::LogProfile)
            }
            LogAct::Name(d) => {
                let Some((at, p)) = &mut self.open else {
                    return Done::Stay;
                };
                let taken: Vec<&String> =
                    names.iter().filter(|n| n.as_str() != at.as_str()).collect();
                let now = NAMES.iter().position(|x| *x == p.name);
                let len = NAMES.len() as i32;
                // A name from the portal starts the cycle from the top.
                let from = now.map_or(if *d > 0 { -1 } else { 0 }, |i| i as i32);
                for k in 1..=len {
                    let next = NAMES[(from + d * k).rem_euclid(len) as usize];
                    if !taken.iter().any(|t| t.as_str() == next) {
                        p.name = next.to_string();
                        break;
                    }
                }
                self.save(client);
                Done::Stay
            }
            LogAct::Rate(d) => {
                let Some((_, p)) = &mut self.open else {
                    return Done::Stay;
                };
                let at = DATALOG_RATES
                    .iter()
                    .position(|r| *r == p.rate_hz)
                    .unwrap_or(0) as i32;
                let to = (at + d).clamp(0, DATALOG_RATES.len() as i32 - 1) as usize;
                if DATALOG_RATES[to] == p.rate_hz {
                    return Done::Stay;
                }
                p.rate_hz = DATALOG_RATES[to];
                self.save(client);
                Done::Stay
            }
            LogAct::Remove => {
                let Some((at, _)) = &self.open else {
                    return Done::Stay;
                };
                if !self.removing {
                    self.removing = true;
                    return Done::Stay;
                }
                client.datalog(DatalogAction::Delete { name: at.clone() });
                if self.pick.as_ref() == Some(at) {
                    self.pick = None;
                }
                self.reset();
                Done::Back
            }
        }
    }

    /// The channel editor's acts on the open profile. True in the second field when the profile
    /// changed (and went to carpid).
    pub fn slots(
        &mut self,
        a: &SlotAct,
        recent: &mut Vec<String>,
        client: &Client,
    ) -> (Focus, bool) {
        let Some((_, p)) = &mut self.open else {
            return (Focus::Stay, false);
        };
        let (focus, changed) = self
            .editor
            .apply_list(a, &mut p.values, recent, &slots::LOG);
        if changed {
            self.save(client);
        }
        (focus, changed)
    }

    /// Sends the open profile to carpid, renamed if its name changed.
    fn save(&mut self, client: &Client) {
        let Some((at, p)) = &mut self.open else {
            return;
        };
        let replaces = (*at != p.name).then(|| at.clone());
        client.datalog(DatalogAction::Save {
            profile: p.clone(),
            replaces: replaces.clone(),
        });
        if let Some(old) = replaces
            && self.pick.as_ref() == Some(&old)
        {
            self.pick = Some(p.name.clone());
        }
        *at = p.name.clone();
    }

    /// The load carpid worked out for the open profile, once it has its latest copy.
    fn load<'a>(&self, snap: &'a Snapshot) -> Option<&'a DatalogLoad> {
        let (_, p) = self.open.as_ref()?;
        snap.datalog
            .as_ref()?
            .profiles
            .iter()
            .find(|x| x.profile == *p)
            .map(|x| &x.load)
    }
}

/// `12 values · 10 Hz`.
fn size(p: &DatalogProfile) -> String {
    let n = p.values.len();
    format!(
        "{n} value{} · {} Hz",
        if n == 1 { "" } else { "s" },
        p.rate_hz
    )
}

/// `1:05` or `1:02:05`.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// `4812` -> `4 812`.
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// What polling the set costs, in one line.
pub fn load_line(l: &DatalogLoad) -> String {
    if l.diag_values == 0 {
        return "No polling: the bus sends all of these".into();
    }
    let mut s = format!(
        "{} diag value{} in {} read{}",
        l.diag_values,
        if l.diag_values == 1 { "" } else { "s" },
        l.diag_groups,
        if l.diag_groups == 1 { "" } else { "s" }
    );
    if l.diag_modules > 1 {
        s.push_str(&format!(" on {} modules", l.diag_modules));
    }
    if let Some(hz) = l.diag_hz {
        s.push_str(&format!(", about {hz} Hz each"));
    }
    s
}

/// The diag values can't keep up with the profile's rate.
fn slow(l: &DatalogLoad, rate: u32) -> bool {
    l.diag_hz.is_some_and(|hz| hz + 0.05 < rate as f64)
}

fn note(snap: &Snapshot) -> String {
    if !snap.connected {
        return snap
            .problem
            .clone()
            .unwrap_or_else(|| "carpid not connected".into());
    }
    snap.datalog_note.clone().unwrap_or_default()
}

fn log(a: LogAct) -> Act {
    Act::Log(a)
}

/// The Logging screen: the run, then the profiles.
pub fn root(state: &Logging, snap: &Snapshot) -> View {
    let mut v = View::new(Layout::List, "Logging", true).note(note(snap));
    let Some(st) = &snap.datalog else {
        v.row(Row::new(RowKind::Info, "Waiting for carpid", "").dimmed(true));
        return v.finish();
    };
    match &st.running {
        Some(r) => {
            let at = v.row(
                Row::new(
                    RowKind::Button,
                    format!("Logging {}", r.label),
                    format!("{} · {} rows", clock(r.elapsed_ms), group(r.rows)),
                )
                .button("Stop")
                .act(log(LogAct::Stop)),
            );
            v.start = at;
            v.row(Row::new(
                RowKind::Info,
                "Profile",
                format!("{} · {} values · {} Hz", r.profile, r.values, r.rate_hz),
            ));
        }
        None => match state.pick(st) {
            Some(p) => {
                let mut row = Row::new(RowKind::Choice, "Profile", p.name.clone());
                if st.profiles.len() > 1 {
                    row = row
                        .act(log(LogAct::Pick(1)))
                        .side(log(LogAct::Pick(-1)), log(LogAct::Pick(1)));
                } else {
                    row.kind = RowKind::Info;
                }
                v.row(row);
                let at = if p.values.is_empty() {
                    v.row(
                        Row::new(RowKind::Info, format!("{} has no channels yet", p.name), "")
                            .dimmed(true),
                    )
                } else if snap.connected {
                    v.row(
                        Row::new(RowKind::Button, "Start logging", size(p))
                            .button("Start")
                            .act(log(LogAct::Start)),
                    )
                } else {
                    -1
                };
                v.start = at;
            }
            None => {
                v.row(Row::new(RowKind::Info, "No profiles yet", "").dimmed(true));
            }
        },
    }
    if let Some(r) = st.last.as_ref().filter(|_| st.running.is_none()) {
        let mut row = Row::new(
            RowKind::Info,
            format!("Last: {}", r.label),
            match &r.error {
                Some(e) => format!("stopped: {e}"),
                None => r.summary.clone(),
            },
        );
        row.alert = r.error.is_some();
        v.row(row);
    }
    v.section("Profiles");
    for p in &st.profiles {
        let mut row = Row::new(RowKind::Route, p.profile.name.clone(), size(&p.profile))
            .act(log(LogAct::Open(p.profile.name.clone())));
        row.alert = slow(&p.load, p.profile.rate_hz);
        v.row(row);
    }
    let names: Vec<&str> = st
        .profiles
        .iter()
        .map(|p| p.profile.name.as_str())
        .collect();
    if NAMES.iter().any(|n| !names.contains(n)) {
        v.row(
            Row::new(RowKind::Button, "Add a profile", "")
                .button("Add")
                .act(log(LogAct::Add)),
        );
    }
    v.finish()
}

/// One profile: name, rate, channels and what they cost.
pub fn profile(state: &Logging, snap: &Snapshot) -> View {
    let Some((_, p)) = &state.open else {
        return View::new(Layout::List, "Profile", true).finish();
    };
    let mut v = View::new(Layout::List, p.name.clone(), true).note(note(snap));
    v.row(
        Row::new(RowKind::Choice, "Name", p.name.clone())
            .act(log(LogAct::Name(1)))
            .side(log(LogAct::Name(-1)), log(LogAct::Name(1))),
    );
    v.row(
        Row::new(RowKind::Step, "Rate", format!("{} Hz", p.rate_hz))
            .side(log(LogAct::Rate(-1)), log(LogAct::Rate(1))),
    );
    let n = p.values.len();
    let channels = match n {
        0 => "none yet".to_string(),
        1 => "1 value".to_string(),
        n => format!("{n} values"),
    };
    v.row(Row::new(RowKind::Route, "Channels", channels).act(Act::Car(CarScreen::LogChannels)));
    if n > 0 {
        let row = match state.load(snap) {
            Some(l) => {
                let mut r = Row::new(RowKind::Info, "Load", load_line(l));
                r.alert = slow(l, p.rate_hz);
                r
            }
            None => Row::new(RowKind::Info, "Load", "…").dimmed(true),
        };
        v.row(row);
    }
    v.section("");
    let mut row = Row::new(
        RowKind::Button,
        if state.removing {
            "Confirm remove"
        } else {
            "Remove this profile"
        },
        "",
    )
    .button("Remove")
    .act(log(LogAct::Remove));
    row.alert = state.removing;
    v.row(row);
    v.finish()
}

/// The open profile's channels: the slot editor's list, and its picker.
pub fn channels(state: &Logging, recent: &[String], snap: &Snapshot, now: Instant) -> View {
    let Some((_, p)) = &state.open else {
        return View::new(Layout::List, "Channels", true).finish();
    };
    let line = match state.load(snap) {
        Some(l) if !p.values.is_empty() => format!("{} Hz · {}", p.rate_hz, load_line(l)),
        _ => String::new(),
    };
    let note = match note(snap) {
        n if n.is_empty() => line,
        n => n,
    };
    let title = format!("{} channels", p.name);
    let mut v = state
        .editor
        .list_view(&title, &p.values, note, recent, snap, now, &slots::LOG);
    if state.editor.picker.is_some() && !snap.connected {
        v.note = self::note(snap);
    }
    v
}

/// Does `a` to `st` the way carpid would, for the fixture.
pub fn apply(st: &mut DatalogStatus, a: &DatalogAction) {
    use carpi_proto::{DatalogProfileStatus, DatalogRun};
    match a {
        DatalogAction::Status => {}
        DatalogAction::Save { profile, replaces } => {
            let name = replaces.as_ref().unwrap_or(&profile.name);
            let entry = DatalogProfileStatus {
                profile: profile.clone(),
                load: DatalogLoad::default(),
            };
            match st.profiles.iter_mut().find(|p| p.profile.name == *name) {
                Some(p) => *p = entry,
                None => st.profiles.push(entry),
            }
        }
        DatalogAction::Delete { name } => st.profiles.retain(|p| p.profile.name != *name),
        DatalogAction::Start { profile } => {
            let Some(p) = st.profiles.iter().find(|p| p.profile.name == *profile) else {
                return;
            };
            if st.running.is_some() {
                return;
            }
            st.selected = Some(profile.clone());
            st.running = Some(DatalogRun {
                label: "2026-11-08 #04".into(),
                date: "2026-11-08".into(),
                number: 4,
                profile: profile.clone(),
                values: p.profile.values.len() as u32,
                rate_hz: p.profile.rate_hz,
                started: "2026-11-08T10:00:00".into(),
                elapsed_ms: 0,
                rows: 0,
                dir: "/home/pi/logs/2026-11-08/04".into(),
                summary: String::new(),
                error: None,
            });
        }
        DatalogAction::Stop => {
            if let Some(mut r) = st.running.take() {
                r.summary = format!("{} values, {} Hz, {} rows", r.values, r.rate_hz, r.rows);
                st.last = Some(r);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use carpi_proto::{DatalogProfileStatus, DatalogRun, Reading, Value};

    use super::*;
    use crate::view::Hit;

    pub(crate) fn status() -> DatalogStatus {
        let p = |name: &str, values: &[&str], rate_hz, load: DatalogLoad| DatalogProfileStatus {
            profile: DatalogProfile {
                name: name.into(),
                values: values.iter().map(|v| v.to_string()).collect(),
                rate_hz,
            },
            load,
        };
        DatalogStatus {
            profiles: vec![
                p(
                    "Boost",
                    &[
                        "engine.rpm",
                        "engine.boost.actual",
                        "engine.boost.specified",
                    ],
                    10,
                    DatalogLoad {
                        bus_values: 1,
                        diag_values: 2,
                        diag_groups: 1,
                        diag_modules: 1,
                        diag_hz: Some(10.0),
                    },
                ),
                p(
                    "Haldex",
                    &["awd.oil_temp_c", "awd.plate_temp_c", "engine.oil_temp_c"],
                    20,
                    DatalogLoad {
                        bus_values: 0,
                        diag_values: 3,
                        diag_groups: 2,
                        diag_modules: 2,
                        diag_hz: Some(13.3),
                    },
                ),
            ],
            selected: Some("Haldex".into()),
            running: None,
            last: None,
        }
    }

    fn snap(st: DatalogStatus) -> Snapshot {
        let mut s = Snapshot {
            connected: true,
            problem: None,
            datalog: Some(st),
            ..Snapshot::default()
        };
        s.values.take(
            vec![Reading {
                name: "engine.rpm".into(),
                value: Some(Value::Num(900.0)),
                age_ms: 0,
            }],
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
    fn the_root_starts_the_picked_profile_and_shows_the_run() {
        let client = Client::offline("test");
        let mut l = Logging::default();
        let mut st = status();
        let v = root(&l, &snap(st.clone()));
        let pick = &v.rows[find(&v, "Profile")];
        assert_eq!(
            (pick.kind, pick.value.as_str()),
            (RowKind::Choice, "Haldex")
        );
        assert_eq!(v.hit(v.start), Some(Hit::Row(find(&v, "Start logging"))));
        assert_eq!(v.rows[find(&v, "Start logging")].value, "3 values · 20 Hz");
        // Haldex can't poll at 20 Hz: flagged.
        assert!(v.rows[find(&v, "Haldex")].alert);
        assert!(!v.rows[find(&v, "Boost")].alert);
        assert!(v.rows.iter().any(|r| r.act == Act::Log(LogAct::Add)));

        // The D-pad picks Boost; Start asks for it.
        l.apply(&LogAct::Pick(1), Some(&st), &client);
        assert_eq!(l.pick(&st).unwrap().name, "Boost");
        l.apply(&LogAct::Start, Some(&st), &client);
        assert_eq!(
            client.snapshot().datalog_note.as_deref(),
            Some("not connected")
        );

        st.running = Some(DatalogRun {
            label: "2026-11-08 #04".into(),
            date: "2026-11-08".into(),
            number: 4,
            profile: "Boost".into(),
            values: 3,
            rate_hz: 10,
            started: "2026-11-08T10:00:00".into(),
            elapsed_ms: 83_400,
            rows: 4812,
            dir: String::new(),
            summary: String::new(),
            error: None,
        });
        let v = root(&l, &snap(st.clone()));
        let run = &v.rows[v.rows.iter().position(|r| r.button == "Stop").unwrap()];
        assert_eq!(run.label, "Logging 2026-11-08 #04");
        assert_eq!(run.value, "1:23 · 4 812 rows");
        assert_eq!(v.hit(v.start), Some(Hit::Row(0)));
        assert!(!v.rows.iter().any(|r| r.label == "Start logging"));

        st.last = st.running.take().map(|mut r| {
            r.summary = "3 values, 10 Hz, 4 812 rows".into();
            r
        });
        let v = root(&l, &snap(st));
        assert_eq!(
            v.rows[find(&v, "Last: 2026-11-08 #04")].value,
            "3 values, 10 Hz, 4 812 rows"
        );
        let v = root(&l, &Snapshot::default());
        assert_eq!(v.rows[0].label, "Waiting for carpid");
    }

    #[test]
    fn profiles_are_added_named_stepped_and_removed() {
        let client = Client::offline("test");
        let mut l = Logging::default();
        let mut st = status();
        assert_eq!(
            l.apply(&LogAct::Add, Some(&st), &client),
            Done::Opened(CarScreen::LogProfile)
        );
        // Engine is the first free name.
        let (at, p) = l.open.clone().unwrap();
        assert_eq!(
            (at.as_str(), p.name.as_str(), p.rate_hz),
            ("Engine", "Engine", 10)
        );
        apply(
            &mut st,
            &DatalogAction::Save {
                profile: p,
                replaces: None,
            },
        );

        // Name skips the taken ones, both ways; carpid hears the rename.
        l.apply(&LogAct::Name(1), Some(&st), &client);
        assert_eq!(l.open.as_ref().unwrap().1.name, "Electrical");
        assert_eq!(l.open.as_ref().unwrap().0, "Electrical");
        apply(
            &mut st,
            &DatalogAction::Save {
                profile: l.open.as_ref().unwrap().1.clone(),
                replaces: Some("Engine".into()),
            },
        );
        l.apply(&LogAct::Name(-1), Some(&st), &client);
        assert_eq!(l.open.as_ref().unwrap().1.name, "Engine");
        assert_eq!(l.open.as_ref().unwrap().0, "Engine");

        // Rate steps and stops at the ends.
        l.apply(&LogAct::Rate(1), Some(&st), &client);
        l.apply(&LogAct::Rate(1), Some(&st), &client);
        assert_eq!(l.open.as_ref().unwrap().1.rate_hz, 20);
        l.apply(&LogAct::Rate(-9), Some(&st), &client);
        assert_eq!(l.open.as_ref().unwrap().1.rate_hz, 1);

        let v = profile(&l, &snap(st.clone()));
        assert_eq!(v.title, "Engine");
        assert_eq!(v.rows[find(&v, "Rate")].kind, RowKind::Step);
        assert_eq!(v.rows[find(&v, "Channels")].value, "none yet");
        assert!(!v.rows.iter().any(|r| r.label == "Load"));

        // Remove asks first.
        assert_eq!(l.apply(&LogAct::Remove, Some(&st), &client), Done::Stay);
        let v = profile(&l, &snap(st.clone()));
        assert!(
            v.rows
                .iter()
                .any(|r| r.label == "Confirm remove" && r.alert)
        );
        assert_eq!(l.apply(&LogAct::Remove, Some(&st), &client), Done::Back);
        assert!(l.open.is_none());
    }

    #[test]
    fn channels_use_the_list_editor_and_show_the_load() {
        let client = Client::offline("test");
        let mut l = Logging::default();
        let st = status();
        l.apply(&LogAct::Open("Boost".into()), Some(&st), &client);
        let s = snap(st.clone());
        let now = Instant::now();
        let v = profile(&l, &s);
        assert_eq!(
            v.rows[find(&v, "Load")].value,
            "2 diag values in 1 read, about 10 Hz each"
        );
        let v = channels(&l, &[], &s, now);
        assert_eq!(v.title, "Boost channels");
        assert_eq!(v.note, "10 Hz · 2 diag values in 1 read, about 10 Hz each");
        assert_eq!(v.rows[0].label, "Engine speed");
        assert_eq!(v.rows[1].tag, "diag");

        // Add one: the picker, then a value.
        let mut recent = Vec::new();
        l.slots(&SlotAct::Open(3), &mut recent, &client);
        l.slots(
            &SlotAct::Cat(slots::Cat::Of("driving")),
            &mut recent,
            &client,
        );
        let (_, changed) = l.slots(
            &SlotAct::Pick("vehicle.speed_kmh".into()),
            &mut recent,
            &client,
        );
        assert!(changed);
        assert_eq!(l.open.as_ref().unwrap().1.values.len(), 4);
        // carpid hasn't sent the new copy's load yet.
        assert_eq!(profile(&l, &s).rows[3].value, "…");
    }

    #[test]
    fn the_fixture_does_what_carpid_does() {
        let mut st = status();
        apply(
            &mut st,
            &DatalogAction::Start {
                profile: "Boost".into(),
            },
        );
        assert_eq!(st.running.as_ref().unwrap().values, 3);
        apply(&mut st, &DatalogAction::Stop);
        assert!(st.running.is_none() && st.last.is_some());
        apply(
            &mut st,
            &DatalogAction::Delete {
                name: "Boost".into(),
            },
        );
        assert_eq!(st.profiles.len(), 1);
        assert_eq!(clock(3_725_000), "1:02:05");
        assert_eq!(group(1_234_567), "1 234 567");
    }
}
