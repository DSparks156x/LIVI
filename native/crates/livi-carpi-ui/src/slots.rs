// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The slot editor and the value picker (docs/PAGES.md 2). One component for the Data page, the
//! DIS readings and data-log channel sets: a [`Board`] is a list of pages with a fixed number of
//! slots each, a [`Spec`] says what a use allows, the [`Editor`] edits a board (or, for a data
//! log, one list without pages or a cap) and the [`Picker`] picks one value. Both build plain
//! [`View`]s and take [`SlotAct`]s.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::client::Snapshot;
use crate::defs::{CATEGORIES, Category, DEFS, describe, hidden, place};
use crate::diag::{MEASURED, NO_POLL};
use crate::values::{NONE, show};
use crate::view::{Act, Cell, Layout, Row, RowKind, View};

/// Most values a page holds, whatever its size.
pub const MAX_SLOTS: usize = 16;
/// Values the picker remembers.
pub const RECENT: usize = 6;

/// What one use of the editor allows.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// Slots per page, the first is the default.
    pub sizes: &'static [usize],
    /// Page names to cycle through.
    pub names: &'static [&'static str],
    /// Categories this use doesn't offer.
    pub without: &'static [&'static str],
    pub max_pages: usize,
    /// One list that grows: the picker removes instead of emptying.
    pub list: bool,
    /// Takes several values at once (Add all four).
    pub many: bool,
}

impl Spec {
    pub fn offers(&self, cat: &str) -> bool {
        !self.without.contains(&cat)
    }

    /// carpid's catalog's categories when it's in, the built-in ones until then.
    fn categories(&self) -> impl Iterator<Item = &'static Category> + '_ {
        crate::catalog::categories()
            .unwrap_or_else(|| CATEGORIES.iter().collect())
            .into_iter()
            .filter(|c| self.offers(c.id))
    }
}

/// The Data page: 4×4, 3×3 or 2×2 tiles; phone and nav values belong on the DIS.
pub const DATA: Spec = Spec {
    sizes: &[16, 9, 4],
    names: &[
        "Engine",
        "Boost",
        "Haldex",
        "Drivetrain",
        "Body",
        "Electrical",
        "Trip",
        "Custom 1",
        "Custom 2",
        "Custom 3",
        "Custom 4",
    ],
    without: &[crate::defs::PHONE],
    // As many tabs as fit the header at 800 px.
    max_pages: 6,
    list: false,
    many: true,
};

/// The cluster display's readings: one picker per slot, everything offered.
pub const DIS: Spec = Spec {
    sizes: &[8],
    names: &[],
    without: &[],
    max_pages: 1,
    list: false,
    many: false,
};

/// A data-log channel set: one list, no pages, no cap, everything offered.
pub const LOG: Spec = Spec {
    sizes: &[],
    names: &[],
    without: &[],
    max_pages: 1,
    list: true,
    many: true,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotPage {
    pub name: String,
    /// Slots shown; `slots` keeps [`MAX_SLOTS`] so shrinking a page loses nothing.
    pub size: usize,
    pub slots: Vec<Option<String>>,
}

impl SlotPage {
    pub fn new(name: &str, size: usize, values: &[&str]) -> Self {
        let mut slots: Vec<Option<String>> = values.iter().map(|v| Some(v.to_string())).collect();
        slots.resize(MAX_SLOTS, None);
        Self {
            name: name.into(),
            size,
            slots,
        }
    }

    /// The shown slots.
    pub fn shown(&self) -> &[Option<String>] {
        &self.slots[..self.size.min(self.slots.len())]
    }

    fn fix(&mut self) {
        self.slots.resize(MAX_SLOTS, None);
        self.size = self.size.clamp(1, MAX_SLOTS);
    }
}

/// Pages of slots, and the one shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub pages: Vec<SlotPage>,
    #[serde(default)]
    pub shown: usize,
}

impl Board {
    /// The Data page's start: today's groups, sized to fit.
    pub fn data() -> Self {
        let pages = crate::defs::PRESETS
            .iter()
            .map(|p| SlotPage::new(p.name, fit(p.values.len()), p.values))
            .collect();
        Self { pages, shown: 0 }
    }

    /// Fixes what a hand-edited file can get wrong.
    pub fn fixed(mut self) -> Self {
        if self.pages.is_empty() {
            return Self::data();
        }
        for p in &mut self.pages {
            p.fix();
        }
        self.shown = self.shown.min(self.pages.len() - 1);
        self
    }

    pub fn page(&self) -> &SlotPage {
        &self.pages[self.shown.min(self.pages.len() - 1)]
    }

    fn page_mut(&mut self) -> &mut SlotPage {
        let i = self.shown.min(self.pages.len() - 1);
        &mut self.pages[i]
    }
}

/// The smallest grid that holds `n` values.
pub fn fit(n: usize) -> usize {
    match n {
        0..=4 => 4,
        5..=9 => 9,
        _ => 16,
    }
}

/// Grid columns for a page of `size` slots.
pub fn cols(size: usize) -> usize {
    match size {
        0..=1 => 1,
        2..=4 => 2,
        5..=9 => 3,
        _ => 4,
    }
}

/// A picker level-2 list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    Recent,
    Of(&'static str),
    /// Module groups: the modules.
    Modules,
    /// One module's groups, by VCDS address.
    Module(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SlotAct {
    /// Shows or edits page `i` (a tab).
    Page(usize),
    /// The D-pad on the tabs or the plain page: the page before or after.
    PageStep(i32),
    /// Opens the picker for a slot.
    Open(usize),
    /// Moves a slot's value one slot along.
    Shift {
        slot: usize,
        by: i32,
    },
    Name(i32),
    Size(i32),
    PageMove(i32),
    PageAdd,
    PageRemove,
    /// Picker level 1: empty the slot, or open a category.
    Empty,
    Cat(Cat),
    /// Picker level 2: the category before or after.
    CatStep(i32),
    Pick(String),
    /// Several values at once: a group's four fields.
    PickAll(Vec<String>),
    /// The module group shown, one up or down.
    GroupStep(i32),
    /// Picker level 2 of a category with sub-groups: open this one.
    Sub(String),
}

fn act(a: SlotAct) -> Act {
    Act::Slots(a)
}

/// Values only diagnostics give, and carpid polls while something watches them.
pub fn polled(name: &str) -> bool {
    if crate::defs::module_group(name).is_some() {
        return true;
    }
    crate::catalog::diag(name)
        .unwrap_or_else(|| MEASURED.iter().any(|m| m.0 == name && m.1 != NO_POLL))
}

/// "3 live · 2 diag": how many of `names` carpid sent, and how many more it polls on demand.
fn count<'a>(names: impl Iterator<Item = &'a str>, snap: &Snapshot) -> (String, bool) {
    let (mut live, mut diag) = (0, 0);
    for n in names {
        if snap.values.get(n).is_some() {
            live += 1;
        } else if polled(n) {
            diag += 1;
        }
    }
    let text = match (live, diag) {
        (0, 0) => "none live".to_string(),
        (l, 0) => format!("{l} live"),
        (0, d) => format!("{d} diag"),
        (l, d) => format!("{l} live · {d} diag"),
    };
    (text, live + diag > 0)
}

/// A category's sub-groups, in member order.
fn subs(list: &[(String, String)]) -> Vec<String> {
    list.iter().fold(Vec::new(), |mut out, (s, _)| {
        if !s.is_empty() && !out.contains(s) {
            out.push(s.clone());
        }
        out
    })
}

/// The names of a category, in picker order: the catalog's, then anything else carpid has that
/// lands there, alphabetically, each with its sub-group.
pub fn members(cat: &str, snap: &Snapshot) -> Vec<(String, String)> {
    let listed = crate::catalog::members(cat);
    let in_catalog = |n: &str| crate::catalog::entry(n).is_some();
    let mut known: Vec<(String, String)> = listed.clone().unwrap_or_default();
    // Ours the catalog doesn't have: the bridge's and the light model's.
    known.extend(
        DEFS.iter()
            .filter(|d| place(d.name).0 == cat && !in_catalog(d.name))
            .map(|d| (d.sub.to_string(), d.name.to_string())),
    );
    let mut extra: Vec<(String, String)> = snap
        .values
        .names()
        .filter(|n| !hidden(n) && crate::defs::find(n).is_none() && !in_catalog(n))
        .filter_map(|n| {
            let (c, sub) = place(n);
            (c == cat).then(|| (sub.into_owned(), n.to_string()))
        })
        .collect();
    extra.sort();
    if listed.is_some() {
        // The catalog's order, then ours by sub-group, then the rest.
        let subs: Vec<String> = known.iter().fold(Vec::new(), |mut out, (s, _)| {
            if !out.contains(s) {
                out.push(s.clone());
            }
            out
        });
        let rank = |s: &str| subs.iter().position(|x| x == s).unwrap_or(subs.len());
        known.sort_by_key(|(s, _)| rank(s));
        known.extend(extra);
        return known;
    }
    // Known sub-groups keep the category's order; new ones go after, alphabetically.
    let subs = crate::defs::category(cat).map_or(&[][..], |c| c.subs);
    let rank = |s: &str| subs.iter().position(|x| *x == s).unwrap_or(subs.len());
    known.extend(extra);
    known.sort_by(|a, b| {
        rank(&a.0)
            .cmp(&rank(&b.0))
            .then_with(|| match rank(&a.0) == subs.len() {
                true => a.0.cmp(&b.0),
                false => std::cmp::Ordering::Equal,
            })
    });
    known
}

/// The value picker for one slot. Level 1 while `cat` is `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picker {
    pub cat: Option<Cat>,
    /// The sub-group open within `cat`.
    pub sub: Option<String>,
    /// The group on a module's screen.
    pub group: u8,
}

/// What a picker act did.
#[derive(Debug, Clone, PartialEq)]
pub enum Picked {
    /// Moved within the picker: focus the view's start.
    Moved,
    Stay,
    /// Done: put this in the slot (`None` empties it).
    Chose(Option<String>),
    /// Done: these, in order (a group's fields).
    ChoseAll(Vec<String>),
}

impl Picker {
    /// The categories the D-pad steps through: Recent when there is any, then the spec's.
    fn order(spec: &Spec, recent: &[String]) -> Vec<Cat> {
        let mut out = Vec::new();
        if !recent.is_empty() {
            out.push(Cat::Recent);
        }
        out.extend(spec.categories().map(|c| Cat::Of(c.id)));
        out
    }

    pub fn apply(&mut self, a: &SlotAct, spec: &Spec, recent: &[String]) -> Picked {
        match a {
            SlotAct::Empty => Picked::Chose(None),
            SlotAct::Pick(name) => Picked::Chose(Some(name.clone())),
            SlotAct::PickAll(names) => Picked::ChoseAll(names.clone()),
            SlotAct::GroupStep(d) => {
                let g = i32::from(self.group.max(1)) + d;
                self.group = (g - 1).rem_euclid(255) as u8 + 1;
                Picked::Stay
            }
            SlotAct::Cat(c) => {
                if matches!(c, Cat::Module(_)) && self.group == 0 {
                    self.group = 1;
                }
                self.cat = Some(*c);
                self.sub = None;
                Picked::Moved
            }
            SlotAct::CatStep(d) => {
                let order = Self::order(spec, recent);
                let Some(now) = self.cat.and_then(|c| order.iter().position(|o| *o == c)) else {
                    return Picked::Stay;
                };
                let n = order.len() as i32;
                self.cat = Some(order[(now as i32 + d).rem_euclid(n) as usize]);
                self.sub = None;
                Picked::Moved
            }
            SlotAct::Sub(s) => {
                self.sub = Some(s.clone());
                Picked::Moved
            }
            _ => Picked::Stay,
        }
    }

    /// One level up; false on level 1, where back leaves the picker.
    pub fn back(&mut self) -> bool {
        if self.sub.take().is_some() {
            return true;
        }
        if matches!(self.cat, Some(Cat::Module(_))) {
            self.cat = Some(Cat::Modules);
            return true;
        }
        self.cat.take().is_some()
    }

    /// Levels below the first: 0, 1, or 2 on a module's groups or a sub-group.
    pub fn level(&self) -> usize {
        match self.cat {
            None => 0,
            Some(Cat::Module(_)) => 2,
            Some(_) if self.sub.is_some() => 2,
            Some(_) => 1,
        }
    }

    /// The module and group a module's screen shows, and the four value names.
    pub fn group_names(&self) -> Option<(u8, u8, Vec<String>)> {
        let Some(Cat::Module(addr)) = self.cat else {
            return None;
        };
        let g = self.group.max(1);
        Some((
            addr,
            g,
            (1..=4)
                .map(|f| crate::defs::module_group_name(addr, g, f))
                .collect(),
        ))
    }

    /// The picker as a view: `title` names the slot, `current` is what's in it.
    pub fn view(
        &self,
        title: &str,
        current: Option<&str>,
        recent: &[String],
        snap: &Snapshot,
        now: Instant,
        spec: &Spec,
    ) -> View {
        let mut v = match self.cat {
            None => level1(title, current, recent, snap, spec),
            Some(Cat::Modules) => modules_view(snap),
            Some(Cat::Module(addr)) => self.group_view(addr, current, snap, now, spec),
            Some(cat) => self.level2(cat, current, recent, snap, now, spec),
        };
        v.custom = true;
        v
    }

    /// One module's group: the number on the D-pad, its four fields live.
    fn group_view(
        &self,
        addr: u8,
        current: Option<&str>,
        snap: &Snapshot,
        now: Instant,
        spec: &Spec,
    ) -> View {
        let g = self.group.max(1);
        let module = snap
            .diag
            .modules()
            .into_iter()
            .find(|m| m.module.address == addr);
        let title = module.as_ref().map_or_else(
            || format!("{addr:02X}"),
            |m| format!("{addr:02X}  {}", m.module.name),
        );
        let mut v = View::new(Layout::List, title, true);
        let at = v.row(
            Row::new(RowKind::Step, "Group", g.to_string())
                .act(act(SlotAct::GroupStep(1)))
                .side(act(SlotAct::GroupStep(-1)), act(SlotAct::GroupStep(1))),
        );
        v.start = at;
        let tp2 = module.map(|m| m.module.tp2);
        let read = tp2.and_then(|t| snap.diag.groups.get(&(t, g)));
        let mut names = Vec::new();
        for f in 1..=4u8 {
            let name = crate::defs::module_group_name(addr, g, f);
            let field = read.and_then(|r| r.get(usize::from(f) - 1));
            let unit = field.map_or("", |x| x.unit.as_str());
            let live = match snap.values.get(&name).and_then(|s| s.value.as_ref()) {
                Some(carpi_proto::Value::Num(n)) => Some(crate::values::number(*n, 2)),
                _ => None,
            };
            let value = match (live, field) {
                (Some(x), _) => reading(&x, unit),
                (None, Some(x)) => reading(&x.text, unit),
                (None, None) => "…".to_string(),
            };
            let mut row = Row::new(RowKind::Info, format!("Field {f}"), value)
                .act(act(SlotAct::Pick(name.clone())))
                .checked(current == Some(name.as_str()));
            row.dimmed = field.is_some_and(|x| x.value.is_none() && x.text.is_empty());
            let _ = now;
            v.row(row);
            names.push(name);
        }
        if spec.many {
            v.row(
                Row::new(RowKind::Button, "Add all four", "")
                    .button("Add")
                    .act(act(SlotAct::PickAll(names))),
            );
        }
        v.finish()
    }

    fn level2(
        &self,
        cat: Cat,
        current: Option<&str>,
        recent: &[String],
        snap: &Snapshot,
        now: Instant,
        spec: &Spec,
    ) -> View {
        let order = Self::order(spec, recent);
        let at = order.iter().position(|c| *c == cat).unwrap_or(0);
        let n = order.len();
        let name_of = |c: Cat| match c {
            Cat::Recent => "Recent",
            Cat::Modules | Cat::Module(_) => "Module groups",
            Cat::Of(id) => crate::defs::category(id).map_or(id, |c| c.label),
        };
        let list: Vec<(String, String)> = match cat {
            Cat::Recent => recent.iter().map(|r| (String::new(), r.clone())).collect(),
            Cat::Modules | Cat::Module(_) => Vec::new(),
            Cat::Of(id) => members(id, snap),
        };
        let groups = subs(&list);
        if let Some(sub) = self.sub.as_ref().filter(|s| groups.contains(s)) {
            // One sub-group: the D-pad steps its siblings.
            let at = groups.iter().position(|g| g == sub).unwrap_or(0);
            let n = groups.len();
            let mut v = View::new(Layout::List, sub.clone(), true);
            if n > 1 {
                let prev = &groups[(at + n - 1) % n];
                let next = &groups[(at + 1) % n];
                v.hint = format!("◀ {prev}    {next} ▶");
                v.side = Some((
                    act(SlotAct::Sub(prev.clone())),
                    act(SlotAct::Sub(next.clone())),
                ));
            }
            let mut start = -1;
            for (_, name) in list.iter().filter(|(g, _)| g == sub) {
                let at = v.row(pick_row(name, current, snap, now));
                if current == Some(name.as_str()) {
                    start = at;
                }
            }
            v.start = start;
            return v.finish();
        }
        let mut v = View::new(Layout::List, name_of(cat), true);
        if n > 1 {
            v.hint = format!(
                "◀ {}    {} ▶",
                name_of(order[(at + n - 1) % n]),
                name_of(order[(at + 1) % n])
            );
        }
        v.side = Some((act(SlotAct::CatStep(-1)), act(SlotAct::CatStep(1))));
        let mut start = -1;
        let mut rows = |v: &mut View, names: &mut dyn Iterator<Item = &String>| {
            for name in names {
                let at = v.row(pick_row(name, current, snap, now));
                if current == Some(name.as_str()) {
                    start = at;
                }
            }
        };
        if groups.len() > 1 {
            // Sub-groups as their own screens; anything without one lists after them.
            let here = current.and_then(|c| list.iter().find(|(_, n)| n == c).map(|(g, _)| g));
            let mut sub_at = -1;
            for g in &groups {
                let names = list.iter().filter(|(x, _)| x == g).map(|(_, n)| n.as_str());
                let (text, any) = count(names, snap);
                let at = v.row(
                    Row::new(RowKind::Route, g.clone(), text)
                        .act(act(SlotAct::Sub(g.clone())))
                        .dimmed(!any),
                );
                if here == Some(g) {
                    sub_at = at;
                }
            }
            if list.iter().any(|(g, _)| g.is_empty()) {
                v.section("");
                rows(
                    &mut v,
                    &mut list.iter().filter(|(g, _)| g.is_empty()).map(|(_, n)| n),
                );
            }
            if sub_at >= 0 {
                start = sub_at;
            }
        } else {
            rows(&mut v, &mut list.iter().map(|(_, n)| n));
        }
        if list.is_empty() {
            v.row(Row::new(RowKind::Info, "Nothing here yet", "").dimmed(true));
        }
        v.start = start;
        v.finish()
    }
}

/// Module groups: the modules carpid can poll (never ABS).
fn modules_view(snap: &Snapshot) -> View {
    let mut v = View::new(Layout::List, "Module groups", true);
    for m in snap.diag.modules() {
        if m.coded == Some(false) || m.module.tp2 == NO_POLL {
            continue;
        }
        v.row(
            Row::new(
                RowKind::Route,
                format!("{:02X}  {}", m.module.address, m.module.name),
                "",
            )
            .act(act(SlotAct::Cat(Cat::Module(m.module.address)))),
        );
    }
    v.finish()
}

fn pick_row(name: &str, current: Option<&str>, snap: &Snapshot, now: Instant) -> Row {
    let seen = snap.values.get(name);
    let s = show(name, seen, now);
    let value = match seen {
        None if polled(name) => String::new(),
        None => "not seen".into(),
        Some(_) => reading(&s.value, &s.unit),
    };
    let mut row = Row::new(RowKind::Info, s.label, value)
        .act(act(SlotAct::Pick(name.to_string())))
        .checked(current == Some(name))
        .dimmed(seen.is_none() && !polled(name));
    if polled(name) {
        row.tag = "diag".into();
    }
    row
}

/// A reading and its unit, as one string.
pub fn reading(value: &str, unit: &str) -> String {
    if unit.is_empty() || value == NONE {
        value.to_string()
    } else {
        format!("{value} {unit}")
    }
}

fn level1(
    title: &str,
    current: Option<&str>,
    recent: &[String],
    snap: &Snapshot,
    spec: &Spec,
) -> View {
    let mut v = View::new(Layout::List, title, true);
    let empty = if !spec.list {
        v.row(
            Row::new(RowKind::Info, "Empty", "")
                .act(act(SlotAct::Empty))
                .checked(current.is_none()),
        )
    } else if current.is_some() {
        let mut row = Row::new(RowKind::Button, "Remove this channel", "")
            .button("Remove")
            .act(act(SlotAct::Empty));
        row.alert = true;
        v.row(row)
    } else {
        -1
    };
    v.start = empty;
    let here = current.map(|c| place(c).0);
    if !recent.is_empty() {
        let at = v.row(
            Row::new(RowKind::Route, "Recent", recent.len().to_string())
                .act(act(SlotAct::Cat(Cat::Recent))),
        );
        if current.is_some() && recent.first().map(String::as_str) == current {
            v.start = at;
        }
    }
    v.section("Categories");
    for c in spec.categories() {
        let names = members(c.id, snap);
        if names.is_empty() {
            continue;
        }
        let (text, any) = count(names.iter().map(|(_, n)| n.as_str()), snap);
        let at = v.row(
            Row::new(RowKind::Route, c.label, text)
                .act(act(SlotAct::Cat(Cat::Of(c.id))))
                .dimmed(!any),
        );
        if here == Some(c.id) && v.start == empty {
            v.start = at;
        }
    }
    v.row(Row::new(RowKind::Route, "Module groups", "").act(act(SlotAct::Cat(Cat::Modules))));
    v.finish()
}

/// Where focus goes after an editor act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The view's start.
    Start,
    Stay,
    /// This focus index.
    At(i32),
}

/// The slot editor over a board.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Editor {
    /// The focused slot.
    pub slot: usize,
    pub picker: Option<Picker>,
    /// "Remove this page" waits for a second push.
    pub removing: bool,
}

impl Editor {
    /// Runs `a`; true in the second field when the board changed and wants saving.
    pub fn apply(
        &mut self,
        a: &SlotAct,
        board: &mut Board,
        recent: &mut Vec<String>,
        spec: &Spec,
    ) -> (Focus, bool) {
        if let Some(p) = &mut self.picker {
            let picked = match p.apply(a, spec, recent) {
                Picked::ChoseAll(names) => Picked::Chose(names.into_iter().next()),
                other => other,
            };
            return match picked {
                Picked::Moved => (Focus::Start, false),
                Picked::Stay | Picked::ChoseAll(_) => (Focus::Stay, false),
                Picked::Chose(name) => {
                    let slot = self.slot;
                    if let Some(n) = &name {
                        remember(recent, n);
                    }
                    if let Some(s) = board.page_mut().slots.get_mut(slot) {
                        *s = name;
                    }
                    self.picker = None;
                    (Focus::Start, true)
                }
            };
        }
        if *a != SlotAct::PageRemove {
            self.removing = false;
        }
        let n = board.pages.len();
        match a {
            SlotAct::Page(i) => {
                board.shown = (*i).min(n - 1);
                self.slot = 0;
                (Focus::Stay, true)
            }
            SlotAct::PageStep(d) => {
                board.shown = (board.shown as i32 + d).clamp(0, n as i32 - 1) as usize;
                self.slot = 0;
                (Focus::At(board.shown as i32), true)
            }
            SlotAct::Open(i) => {
                self.slot = *i;
                self.picker = Some(Picker::default());
                (Focus::Start, false)
            }
            SlotAct::Shift { slot, by } => {
                let size = board.page().size;
                let to = *slot as i32 + by;
                if to < 0 || to >= size as i32 {
                    return (Focus::Stay, false);
                }
                board.page_mut().slots.swap(*slot, to as usize);
                self.slot = to as usize;
                (Focus::Start, true)
            }
            SlotAct::Name(d) => {
                let names = spec.names;
                if names.is_empty() {
                    return (Focus::Stay, false);
                }
                let taken: Vec<String> = board
                    .pages
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != board.shown)
                    .map(|(_, p)| p.name.clone())
                    .collect();
                let now = names
                    .iter()
                    .position(|x| *x == board.page().name)
                    .unwrap_or(0) as i32;
                let len = names.len() as i32;
                for k in 1..=len {
                    let next = names[(now + d * k).rem_euclid(len) as usize];
                    if !taken.iter().any(|t| t == next) {
                        board.page_mut().name = next.to_string();
                        break;
                    }
                }
                (Focus::Stay, true)
            }
            SlotAct::Size(d) => {
                let sizes = spec.sizes;
                let now = sizes
                    .iter()
                    .position(|s| *s == board.page().size)
                    .unwrap_or(0) as i32;
                let next = sizes[(now + d).rem_euclid(sizes.len() as i32) as usize];
                board.page_mut().size = next;
                self.slot = self.slot.min(next - 1);
                (Focus::Stay, true)
            }
            SlotAct::PageMove(d) => {
                let to = board.shown as i32 + d;
                if to < 0 || to >= n as i32 {
                    return (Focus::Stay, false);
                }
                board.pages.swap(board.shown, to as usize);
                board.shown = to as usize;
                (Focus::Stay, true)
            }
            SlotAct::PageAdd => {
                if n >= spec.max_pages {
                    return (Focus::Stay, false);
                }
                let name = spec
                    .names
                    .iter()
                    .find(|x| !board.pages.iter().any(|p| p.name == **x))
                    .copied()
                    .unwrap_or("Custom");
                let size = spec.sizes.first().copied().unwrap_or(MAX_SLOTS);
                board
                    .pages
                    .insert(board.shown + 1, SlotPage::new(name, size, &[]));
                board.shown += 1;
                self.slot = 0;
                (Focus::Start, true)
            }
            SlotAct::PageRemove => {
                if n <= 1 {
                    return (Focus::Stay, false);
                }
                if !self.removing {
                    self.removing = true;
                    return (Focus::Stay, false);
                }
                self.removing = false;
                board.pages.remove(board.shown);
                board.shown = board.shown.min(board.pages.len() - 1);
                self.slot = 0;
                (Focus::Start, true)
            }
            _ => (Focus::Stay, false),
        }
    }

    /// One level up: the picker's, then out of it. False when the editor itself closes.
    pub fn back(&mut self) -> bool {
        self.removing = false;
        match &mut self.picker {
            Some(p) => {
                if !p.back() {
                    self.picker = None;
                }
                true
            }
            None => false,
        }
    }

    pub fn view(
        &self,
        board: &Board,
        recent: &[String],
        snap: &Snapshot,
        now: Instant,
        spec: &Spec,
    ) -> View {
        let page = board.page();
        if let Some(p) = &self.picker {
            let current = page.slots.get(self.slot).cloned().flatten();
            let title = format!("Slot {}", self.slot + 1);
            return p.view(&title, current.as_deref(), recent, snap, now, spec);
        }
        let mut v = View::new(Layout::List, "Edit", true);
        if board.pages.len() <= 4 {
            v.hint = "◀ ▶ moves a tile".into();
        }
        for (i, p) in board.pages.iter().enumerate() {
            v.pill(p.name.clone(), i == board.shown, act(SlotAct::Page(i)))
                .side = Some((act(SlotAct::PageStep(-1)), act(SlotAct::PageStep(1))));
        }
        v.cols = cols(page.size);
        v.outlined = true;
        for (i, slot) in page.shown().iter().enumerate() {
            let mut c = tile(slot.as_deref(), snap, now);
            c.act = act(SlotAct::Open(i));
            c.side = Some((
                act(SlotAct::Shift { slot: i, by: -1 }),
                act(SlotAct::Shift { slot: i, by: 1 }),
            ));
            let at = v.cell(c, true);
            if i == self.slot {
                v.start = at;
            }
        }
        v.section("This page");
        let both = |a: fn(i32) -> SlotAct| (act(a(-1)), act(a(1)));
        if !spec.names.is_empty() {
            let (l, r) = both(SlotAct::Name);
            v.row(
                Row::new(RowKind::Choice, "Name", page.name.clone())
                    .act(act(SlotAct::Name(1)))
                    .side(l, r),
            );
        }
        if spec.sizes.len() > 1 {
            let c = cols(page.size);
            let (l, r) = both(SlotAct::Size);
            v.row(
                Row::new(RowKind::Choice, "Tiles", format!("{c} × {c}"))
                    .act(act(SlotAct::Size(1)))
                    .side(l, r),
            );
        }
        if board.pages.len() > 1 {
            let (l, r) = both(SlotAct::PageMove);
            v.row(
                Row::new(
                    RowKind::Step,
                    "Position",
                    format!("{} of {}", board.shown + 1, board.pages.len()),
                )
                .side(l, r),
            );
        }
        if board.pages.len() < spec.max_pages {
            v.row(
                Row::new(RowKind::Button, "Add a page after this one", "")
                    .button("Add")
                    .act(act(SlotAct::PageAdd)),
            );
        }
        if board.pages.len() > 1 {
            let mut row = Row::new(
                RowKind::Button,
                if self.removing {
                    "Confirm remove"
                } else {
                    "Remove this page"
                },
                "",
            )
            .button("Remove")
            .act(act(SlotAct::PageRemove));
            row.alert = self.removing;
            v.row(row);
        }
        v.finish()
    }
}

impl Editor {
    /// Runs `a` on a list (a data-log channel set): slot `list.len()` is the "add" row, and the
    /// picker's Empty removes. True in the second field when the list changed.
    pub fn apply_list(
        &mut self,
        a: &SlotAct,
        list: &mut Vec<String>,
        recent: &mut Vec<String>,
        spec: &Spec,
    ) -> (Focus, bool) {
        if let Some(p) = &mut self.picker {
            return match p.apply(a, spec, recent) {
                Picked::Moved => (Focus::Start, false),
                Picked::Stay => (Focus::Stay, false),
                Picked::ChoseAll(names) => {
                    self.picker = None;
                    let mut changed = false;
                    for n in names {
                        remember(recent, &n);
                        if !list.contains(&n) {
                            list.push(n);
                            changed = true;
                        }
                    }
                    self.slot = list.len().saturating_sub(1);
                    (Focus::Start, changed)
                }
                Picked::Chose(name) => {
                    self.picker = None;
                    let slot = self.slot;
                    let changed = match name {
                        None if slot < list.len() => {
                            list.remove(slot);
                            self.slot = slot.min(list.len());
                            true
                        }
                        None => false,
                        Some(n) => {
                            remember(recent, &n);
                            match list.iter().position(|x| *x == n) {
                                // Already in the set: go to it.
                                Some(at) => {
                                    self.slot = at;
                                    false
                                }
                                None if slot < list.len() => {
                                    list[slot] = n;
                                    true
                                }
                                None => {
                                    list.push(n);
                                    self.slot = list.len() - 1;
                                    true
                                }
                            }
                        }
                    };
                    (Focus::Start, changed)
                }
            };
        }
        match a {
            SlotAct::Open(i) => {
                self.slot = (*i).min(list.len());
                self.picker = Some(Picker::default());
                (Focus::Start, false)
            }
            SlotAct::Shift { slot, by } => {
                let to = *slot as i32 + by;
                if *slot >= list.len() || to < 0 || to >= list.len() as i32 {
                    return (Focus::Stay, false);
                }
                list.swap(*slot, to as usize);
                self.slot = to as usize;
                (Focus::Start, true)
            }
            _ => (Focus::Stay, false),
        }
    }

    /// The list, or the picker for one of its rows. `note` goes under the header.
    #[allow(clippy::too_many_arguments)]
    pub fn list_view(
        &self,
        title: &str,
        list: &[String],
        note: String,
        recent: &[String],
        snap: &Snapshot,
        now: Instant,
        spec: &Spec,
    ) -> View {
        if let Some(p) = &self.picker {
            let current = list.get(self.slot).map(String::as_str);
            let title = match current {
                Some(_) => format!("Channel {}", self.slot + 1),
                None => "New channel".to_string(),
            };
            return p.view(&title, current, recent, snap, now, spec);
        }
        let mut v = View::new(Layout::List, title, true).note(note);
        if list.len() > 1 {
            v.hint = "◀ ▶ moves a channel".into();
        }
        for (i, name) in list.iter().enumerate() {
            let mut row = pick_row(name, None, snap, now);
            row.act = act(SlotAct::Open(i));
            row.side = Some((
                act(SlotAct::Shift { slot: i, by: -1 }),
                act(SlotAct::Shift { slot: i, by: 1 }),
            ));
            let at = v.row(row);
            if i == self.slot {
                v.start = at;
            }
        }
        if list.is_empty() {
            v.row(Row::new(RowKind::Info, "No channels yet", "").dimmed(true));
        }
        v.section("");
        let add = v
            .row(Row::new(RowKind::Route, "Add a channel", "").act(act(SlotAct::Open(list.len()))));
        if self.slot >= list.len() {
            v.start = add;
        }
        v.finish()
    }
}

/// Puts `name` first in `recent`, keeping [`RECENT`].
pub fn remember(recent: &mut Vec<String>, name: &str) {
    recent.retain(|r| r != name);
    recent.insert(0, name.to_string());
    recent.truncate(RECENT);
}

/// A slot's tile: label and reading, or one line why there is none.
pub fn tile(name: Option<&str>, snap: &Snapshot, now: Instant) -> Cell {
    let Some(name) = name else {
        return Cell::blank();
    };
    let seen = snap.values.get(name);
    let s = show(name, seen, now);
    let why = match seen {
        None if polled(name) && !snap.connected => "no data".to_string(),
        None if polled(name) => "polling…".to_string(),
        None => "not seen".to_string(),
        Some(x) if x.value.is_none() => "no value".to_string(),
        Some(_) => String::new(),
    };
    Cell {
        label: s.label,
        value: if why.is_empty() {
            s.value
        } else {
            String::new()
        },
        unit: s.unit,
        why,
        dim: s.freshness.dim(),
        empty: false,
        focus_at: -1,
        act: Act::None,
        side: None,
    }
}

/// The label of a name, for titles.
pub fn label(name: &str) -> String {
    describe(name).label.into_owned()
}

#[cfg(test)]
mod tests {
    use carpi_proto::{Reading, Value};

    use super::*;
    use crate::view::Hit;

    fn snap(names: &[&str]) -> Snapshot {
        let mut s = Snapshot {
            connected: true,
            problem: None,
            ..Snapshot::default()
        };
        s.values.take(
            names
                .iter()
                .map(|n| Reading {
                    name: n.to_string(),
                    value: Some(Value::Num(1.0)),
                    age_ms: 0,
                })
                .collect(),
            Instant::now(),
        );
        s
    }

    #[test]
    fn the_data_board_starts_with_todays_groups() {
        let b = Board::data();
        let names: Vec<&str> = b.pages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Engine", "Drivetrain", "Body", "Electrical"]);
        assert_eq!(b.pages[0].size, 9);
        assert_eq!(b.pages[1].size, 16);
        assert_eq!(b.pages[0].slots.len(), MAX_SLOTS);
        assert_eq!(b.page().shown()[0].as_deref(), Some("engine.rpm"));
    }

    #[test]
    fn members_keep_the_curated_order_and_put_new_names_last() {
        let s = snap(&["engine.zz_new_c", "trip.avg_kmh", "trip.used_l", "foo.bar"]);
        let engine = members("engine", &s);
        assert_eq!(engine[0].1, "engine.rpm");
        assert_eq!(engine.last().unwrap().1, "engine.timing_retard.cyl4_deg");
        let other = members(crate::defs::OTHER, &s);
        let names: Vec<&str> = other.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(
            names,
            ["engine.zz_new_c", "foo.bar", "trip.avg_kmh", "trip.used_l"]
        );
        assert_eq!(other[0].0, "Engine");
        let boost = engine
            .iter()
            .position(|m| m.1 == "engine.boost.actual")
            .unwrap();
        assert_eq!(engine[boost + 1].1, "engine.boost.specified");
    }

    #[test]
    fn level_one_counts_and_starts_on_the_current_category() {
        let s = snap(&["engine.rpm", "engine.coolant_c", "body.door_fl_open"]);
        let p = Picker::default();
        let v = p.view(
            "Slot 1",
            Some("body.door_fl_open"),
            &[],
            &s,
            Instant::now(),
            &DATA,
        );
        assert_eq!(v.rows[0].label, "Empty");
        assert!(!v.rows.iter().any(|r| r.label == "Recent"));
        let engine = v.rows.iter().find(|r| r.label == "Engine").unwrap();
        assert_eq!(engine.value, "2 live · 20 diag");
        assert!(!v.rows.iter().any(|r| r.label == "Phone & nav"));
        let Some(Hit::Row(i)) = v.hit(v.start) else {
            panic!()
        };
        assert_eq!(v.rows[i].label, "Body");
        let v = p.view(
            "Slot 1",
            None,
            &["engine.rpm".into()],
            &s,
            Instant::now(),
            &DIS,
        );
        assert_eq!(v.rows[1].label, "Recent");
        assert!(v.rows.iter().any(|r| r.label == "Phone & nav"));
        assert_eq!(v.start, 0);
    }

    #[test]
    fn level_two_lists_sub_groups_and_steps_categories() {
        let s = snap(&["vehicle.speed_kmh", "steering.angle_deg"]);
        let mut p = Picker::default();
        let recent = vec!["engine.rpm".to_string()];
        assert_eq!(
            p.apply(&SlotAct::Cat(Cat::Of("driving")), &DATA, &recent),
            Picked::Moved
        );
        let now = Instant::now();
        let v = p.view("t", Some("steering.angle_deg"), &recent, &s, now, &DATA);
        assert_eq!(v.title, "Driving");
        assert_eq!(v.hint, "◀ Engine    Haldex ▶");
        let speed = v.rows.iter().find(|r| r.label == "Speed").unwrap();
        assert_eq!(speed.act, act(SlotAct::Sub("Speed".into())));
        assert_eq!(speed.value, "1 live");
        assert!(v.rows.iter().any(|r| r.label == "Wheels"));
        assert!(!v.rows.iter().any(|r| r.label == "Steering angle"));
        let Some(Hit::Row(i)) = v.hit(v.start) else {
            panic!()
        };
        assert_eq!(v.rows[i].label, "Steering");

        // A sub-group: every value, unseen ones dimmed; the D-pad steps sub-groups.
        assert_eq!(
            p.apply(&SlotAct::Sub("Wheels".into()), &DATA, &recent),
            Picked::Moved
        );
        assert_eq!(p.level(), 2);
        let v = p.view("t", None, &recent, &s, now, &DATA);
        assert_eq!(v.title, "Wheels");
        assert!(v.rows.iter().any(|r| r.label == "Pulses FL" && r.dimmed));
        assert!(!v.rows.iter().any(|r| r.label == "Speed"));
        assert!(v.side.is_some());
        assert!(p.back());
        assert_eq!((p.cat, p.sub.as_deref()), (Some(Cat::Of("driving")), None));

        // Wraps from the first category to Recent and on to the last.
        p.cat = Some(Cat::Of("engine"));
        p.apply(&SlotAct::CatStep(-1), &DATA, &recent);
        assert_eq!(p.cat, Some(Cat::Recent));
        p.apply(&SlotAct::CatStep(-1), &DATA, &recent);
        assert_eq!(p.cat, Some(Cat::Of(crate::defs::OTHER)));
        let v = p.view("t", None, &recent, &s, now, &DATA);
        assert_eq!(v.rows[0].label, "Nothing here yet");
    }

    #[test]
    fn diag_values_show_with_a_tag() {
        let s = snap(&[]);
        let p = Picker {
            cat: Some(Cat::Of("haldex")),
            sub: Some("Temps".into()),
            group: 0,
        };
        let v = p.view("t", None, &[], &s, Instant::now(), &DATA);
        let oil = v.rows.iter().find(|r| r.label == "Haldex oil").unwrap();
        assert_eq!((oil.tag.as_str(), oil.value.as_str()), ("diag", ""));
    }

    #[test]
    fn the_editor_picks_reorders_and_manages_pages() {
        let s = snap(&["engine.rpm"]);
        let mut b = Board::data();
        let mut recent = Vec::new();
        let mut e = Editor::default();
        let now = Instant::now();
        let v = e.view(&b, &recent, &s, now, &DATA);
        assert_eq!(v.pills.len(), 4);
        assert_eq!(v.cells.len(), 9);
        assert_eq!(v.start, 4);
        assert!(v.outlined);

        // Push a slot, pick from a category.
        e.apply(&SlotAct::Open(8), &mut b, &mut recent, &DATA);
        e.apply(&SlotAct::Cat(Cat::Of("engine")), &mut b, &mut recent, &DATA);
        let v = e.view(&b, &recent, &s, now, &DATA);
        assert_eq!(v.title, "Engine");
        let (f, save) = e.apply(
            &SlotAct::Pick("engine.boost.actual".into()),
            &mut b,
            &mut recent,
            &DATA,
        );
        assert_eq!((f, save), (Focus::Start, true));
        assert!(e.picker.is_none());
        assert_eq!(b.pages[0].slots[8].as_deref(), Some("engine.boost.actual"));
        assert_eq!(recent, ["engine.boost.actual"]);
        let v = e.view(&b, &recent, &s, now, &DATA);
        assert_eq!(v.start, 4 + 8);

        // The D-pad moves it left.
        e.apply(
            &SlotAct::Shift { slot: 8, by: -1 },
            &mut b,
            &mut recent,
            &DATA,
        );
        assert_eq!(b.pages[0].slots[7].as_deref(), Some("engine.boost.actual"));
        assert_eq!(e.slot, 7);
        let (_, moved) = e.apply(
            &SlotAct::Shift { slot: 0, by: -1 },
            &mut b,
            &mut recent,
            &DATA,
        );
        assert!(!moved, "nothing left of the first slot");

        // Name and size cycle; a shrunk page keeps its values.
        e.apply(&SlotAct::Name(1), &mut b, &mut recent, &DATA);
        assert_eq!(b.pages[0].name, "Boost");
        e.apply(&SlotAct::Size(1), &mut b, &mut recent, &DATA);
        assert_eq!(b.pages[0].size, 4);
        assert_eq!(b.pages[0].slots[7].as_deref(), Some("engine.boost.actual"));
        e.apply(&SlotAct::Size(-1), &mut b, &mut recent, &DATA);
        assert_eq!(b.pages[0].size, 9);

        // Add, move, and remove (twice to confirm).
        e.apply(&SlotAct::PageAdd, &mut b, &mut recent, &DATA);
        assert_eq!(b.pages.len(), 5);
        assert_eq!((b.shown, b.pages[1].name.as_str()), (1, "Engine"));
        e.apply(&SlotAct::PageMove(-1), &mut b, &mut recent, &DATA);
        assert_eq!((b.shown, b.pages[0].name.as_str()), (0, "Engine"));
        e.apply(&SlotAct::PageRemove, &mut b, &mut recent, &DATA);
        assert_eq!(b.pages.len(), 5);
        let v = e.view(&b, &recent, &s, now, &DATA);
        assert!(v.rows.iter().any(|r| r.label == "Confirm remove"));
        e.apply(&SlotAct::PageRemove, &mut b, &mut recent, &DATA);
        assert_eq!(b.pages.len(), 4);
        assert_eq!(b.pages[0].name, "Boost");

        // Back leaves the picker one level at a time.
        e.apply(&SlotAct::Open(0), &mut b, &mut recent, &DATA);
        e.apply(&SlotAct::Cat(Cat::Recent), &mut b, &mut recent, &DATA);
        assert!(e.back());
        assert!(e.picker.as_ref().is_some_and(|p| p.cat.is_none()));
        assert!(e.back());
        assert!(e.picker.is_none());
        assert!(!e.back());
    }

    #[test]
    fn the_list_editor_adds_moves_replaces_and_removes() {
        let s = snap(&["engine.rpm", "vehicle.speed_kmh"]);
        let now = Instant::now();
        let mut list = vec!["engine.rpm".to_string()];
        let mut recent = Vec::new();
        let mut e = Editor::default();
        let v = e.list_view("Channels", &list, String::new(), &recent, &s, now, &LOG);
        assert_eq!(v.rows[0].label, "Engine speed");
        assert_eq!(v.rows.last().unwrap().label, "Add a channel");
        assert_eq!(v.hit(v.start), Some(Hit::Row(0)));

        // Add: the picker has no Empty and nothing to remove.
        e.apply_list(&SlotAct::Open(1), &mut list, &mut recent, &LOG);
        let v = e.list_view("Channels", &list, String::new(), &recent, &s, now, &LOG);
        assert_eq!(v.title, "New channel");
        assert!(
            !v.rows
                .iter()
                .any(|r| r.label == "Empty" || r.act == act(SlotAct::Empty))
        );
        e.apply_list(
            &SlotAct::Cat(Cat::Of("driving")),
            &mut list,
            &mut recent,
            &LOG,
        );
        let (f, changed) = e.apply_list(
            &SlotAct::Pick("vehicle.speed_kmh".into()),
            &mut list,
            &mut recent,
            &LOG,
        );
        assert_eq!((f, changed), (Focus::Start, true));
        assert_eq!(list, ["engine.rpm", "vehicle.speed_kmh"]);
        assert_eq!(e.slot, 1);

        // Picking what's already there just goes to it.
        e.apply_list(&SlotAct::Open(2), &mut list, &mut recent, &LOG);
        let (_, changed) = e.apply_list(
            &SlotAct::Pick("engine.rpm".into()),
            &mut list,
            &mut recent,
            &LOG,
        );
        assert!(!changed);
        assert_eq!((list.len(), e.slot), (2, 0));

        // The D-pad moves a channel; not past the ends.
        e.apply_list(
            &SlotAct::Shift { slot: 0, by: 1 },
            &mut list,
            &mut recent,
            &LOG,
        );
        assert_eq!(list, ["vehicle.speed_kmh", "engine.rpm"]);
        let (_, moved) = e.apply_list(
            &SlotAct::Shift { slot: 1, by: 1 },
            &mut list,
            &mut recent,
            &LOG,
        );
        assert!(!moved);

        // Push a channel: Remove is first and takes it out.
        e.apply_list(&SlotAct::Open(0), &mut list, &mut recent, &LOG);
        let v = e.list_view("Channels", &list, String::new(), &recent, &s, now, &LOG);
        assert_eq!(v.title, "Channel 1");
        assert_eq!(v.rows[0].label, "Remove this channel");
        e.apply_list(&SlotAct::Empty, &mut list, &mut recent, &LOG);
        assert_eq!(list, ["engine.rpm"]);
        assert!(e.picker.is_none());
        assert_eq!(recent, ["engine.rpm", "vehicle.speed_kmh"]);
    }

    #[test]
    fn recent_keeps_six_newest_first() {
        let mut r = Vec::new();
        for i in 0..8 {
            remember(&mut r, &format!("v.{i}"));
        }
        remember(&mut r, "v.5");
        assert_eq!(r, ["v.5", "v.7", "v.6", "v.4", "v.3", "v.2"]);
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    #[test]
    fn module_groups_step_and_pick_fields() {
        let s = Snapshot::default();
        let mut p = Picker::default();
        let v = p.view("Add value", None, &[], &s, Instant::now(), &DATA);
        let at = v
            .rows
            .iter()
            .position(|r| r.label == "Module groups")
            .unwrap();
        assert_eq!(v.rows[at].act, Act::Slots(SlotAct::Cat(Cat::Modules)));
        p.apply(&SlotAct::Cat(Cat::Modules), &DATA, &[]);
        let v = p.view("Add value", None, &[], &s, Instant::now(), &DATA);
        assert!(
            v.rows.iter().all(|r| !r.label.starts_with("03")),
            "never ABS"
        );
        p.apply(&SlotAct::Cat(Cat::Module(0x01)), &DATA, &[]);
        assert_eq!(p.level(), 2);
        p.apply(&SlotAct::GroupStep(-1), &DATA, &[]);
        assert_eq!(p.group, 255);
        p.apply(&SlotAct::GroupStep(103), &DATA, &[]);
        assert_eq!(p.group, 103);
        let v = p.view("Add value", None, &[], &s, Instant::now(), &DATA);
        assert_eq!(v.title, "01  Engine");
        assert_eq!(v.rows[0].value, "103");
        assert_eq!(
            v.rows[2].act,
            Act::Slots(SlotAct::Pick("diag.01.103.2".into()))
        );
        assert!(v.rows.iter().any(|r| r.label == "Add all four"));
        assert!(polled("diag.01.103.2"));
        assert_eq!(crate::defs::describe("diag.01.103.2").label, "01 · 103 · 2");
        assert!(p.back());
        assert_eq!(p.cat, Some(Cat::Modules));
        assert!(p.back());
        assert!(!p.back());
    }
}
