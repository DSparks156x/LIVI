// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Data page (docs/PAGES.md 5c): the tabs Panels, Logs and Faults, and the Panels tab's
//! screens: the panel, its menu, the editor, a widget's settings, the value picker, the list.
//!
//! Viewing, the panel takes no focus of its own: the knob flips panels (livi-ui sees
//! [`View::flip`]) and push opens the menu. Editing, the knob walks the widgets and free cells,
//! push opens one, the D-pad moves a widget.

use std::time::Instant;

use carpi_proto::{DatalogAction, DatalogProfile, DiagAction};

use super::{Moved, Pages, module, modules, problem};
use crate::client::{Client, Snapshot};
use crate::defs;
use crate::logging::{self, LogScreen};
use crate::panels::{
    self, Empty, Grid, MinMax, Panel, PanelAct, PanelScreen, Peak, Style, Target, Widget,
    absolute_pressure, units_for,
};
use crate::slots::{self, Picked, SlotAct, polled};
use crate::view::{Act, DataTab, Layout, Row, RowKind, View};

/// The rate Log this panel logs at.
const PANEL_HZ: u32 = 10;
/// Graph windows, seconds.
const WINDOWS: [u16; 3] = [10, 30, 60];

/// The tabs, as the header's first pills. The D-pad on one steps to the next.
pub(super) fn tabs(v: &mut View, now: DataTab) {
    let all = DataTab::ALL;
    for (i, t) in all.into_iter().enumerate() {
        let prev = all[(i + all.len() - 1) % all.len()];
        let next = all[(i + 1) % all.len()];
        v.pill(t.label(), t == now, Act::Tab(t)).side = Some((Act::Tab(prev), Act::Tab(next)));
    }
}

/// A tab's first screen, with the tabs in its header.
fn tabbed(title: impl Into<String>, now: DataTab) -> View {
    let mut v = View::new(Layout::List, title, false);
    tabs(&mut v, now);
    v
}

fn pa(a: PanelAct) -> Act {
    Act::Panel(a)
}

fn sides(l: PanelAct, r: PanelAct) -> Option<(Act, Act)> {
    Some((pa(l), pa(r)))
}

/// A choice row the D-pad steps both ways and push steps on.
fn choice(label: &str, value: impl Into<String>, mk: impl Fn(i32) -> PanelAct) -> Row {
    Row::new(RowKind::Choice, label, value)
        .act(pa(mk(1)))
        .side(pa(mk(-1)), pa(mk(1)))
}

/// A number the D-pad steps.
fn step(label: &str, value: impl Into<String>, mk: impl Fn(i32) -> PanelAct) -> Row {
    Row::new(RowKind::Step, label, value)
        .act(pa(mk(1)))
        .side(pa(mk(-1)), pa(mk(1)))
}

fn cycle<T: Copy + PartialEq>(all: &[T], now: T, d: i32) -> T {
    let at = all.iter().position(|x| *x == now).unwrap_or(0) as i32;
    all[(at + d).rem_euclid(all.len() as i32) as usize]
}

/// `02:14` while a run goes.
fn rec(snap: &Snapshot) -> String {
    snap.datalog
        .as_ref()
        .and_then(|d| d.running.as_ref())
        .map(|r| logging::clock(r.elapsed_ms))
        .unwrap_or_default()
}

impl Pages {
    pub(super) fn data_depth(&self) -> usize {
        match self.tab {
            DataTab::Panels => {
                let picking = if matches!(self.panel, PanelScreen::Pick(_)) {
                    self.pick.level()
                } else {
                    0
                };
                self.panel.depth() + picking
            }
            DataTab::Logs => self.log.depth(),
            DataTab::Faults => usize::from(self.module.is_some()),
        }
    }

    pub(super) fn data_back(&mut self) -> Option<Moved> {
        match self.tab {
            DataTab::Panels => {
                if matches!(self.panel, PanelScreen::Pick(_)) && self.pick.back() {
                    return Some(Moved::Opened);
                }
                self.panel = self.panel.parent()?;
                self.dropping = false;
                self.pick = Default::default();
                Some(Moved::Closed(self.trail.pop().unwrap_or(0)))
            }
            DataTab::Logs => {
                let picking =
                    self.log.screen == LogScreen::Channels && self.log.editor.picker.is_some();
                if !self.log.back() {
                    return None;
                }
                if picking {
                    return Some(Moved::Opened);
                }
                Some(Moved::Closed(self.trail.pop().unwrap_or(0)))
            }
            DataTab::Faults => {
                self.module.take()?;
                self.confirm = None;
                Some(Moved::Closed(self.trail.pop().unwrap_or(0)))
            }
        }
    }

    /// Runs what waits on carpid: the next module of Clear all once the last job is done, and a
    /// read of the module group a picker shows (for its units); learns module-group units.
    pub fn tick(&mut self, client: &Client) {
        let snap = client.snapshot().clone();
        for ((tp2, g), fields) in &snap.diag.groups {
            let Some(m) = snap.diag.modules().into_iter().find(|m| m.module.tp2 == *tp2) else {
                continue;
            };
            for (f, x) in fields.iter().enumerate().take(4) {
                let name = defs::module_group_name(m.module.address, *g, f as u8 + 1);
                if !x.unit.is_empty() {
                    defs::learn_unit(&name, &x.unit);
                    if self.saved.units.get(&name) != Some(&x.unit) {
                        self.saved.units.insert(name, x.unit.clone());
                    }
                }
            }
        }
        if snap.diag.busy() || !snap.connected {
            return;
        }
        if let Some(m) = self.clearing.pop() {
            client.diag(DiagAction::ClearDtcs {
                module: m,
                confirm: true,
            });
            return;
        }
        let shown = [&self.pick, &self.reading]
            .into_iter()
            .chain(self.log.editor.picker.as_ref())
            .find_map(|p| p.group_names());
        if let Some((addr, g, _)) = shown
            && let Some(m) = snap.diag.modules().into_iter().find(|m| m.module.address == addr)
            && !snap.diag.groups.contains_key(&(m.module.tp2, g))
            && self.asked != Some((m.module.tp2, g))
        {
            self.asked = Some((m.module.tp2, g));
            client.diag(DiagAction::ReadGroup {
                module: m.module.tp2,
                group: g,
            });
        }
    }

    /// The four values of a module group a picker shows, so carpid polls them.
    pub(super) fn picker_watch(&self) -> Vec<String> {
        [&self.pick, &self.reading]
            .into_iter()
            .chain(self.log.editor.picker.as_ref())
            .find_map(|p| p.group_names())
            .map(|(_, _, names)| names)
            .unwrap_or_default()
    }

    /// The values the shown panel needs carpid to poll.
    pub(super) fn panel_watch(&self) -> Vec<String> {
        panels::sampled(self.panels.panel())
            .into_iter()
            .filter(|n| polled(n))
            .collect()
    }

    /// The Data page's own acts; `None` for the shared ones.
    pub(super) fn data_run(&mut self, act: &Act, focus: i32, client: &Client) -> Option<Moved> {
        match act {
            Act::Tab(t) => {
                if *t != self.tab {
                    self.tab = *t;
                    self.panel = PanelScreen::View;
                    self.module = None;
                    self.confirm = None;
                    self.log.reset();
                    self.trail.clear();
                    if *t == DataTab::Faults {
                        let busy = client.snapshot().diag.busy();
                        if !busy {
                            client.diag(DiagAction::Scan);
                        }
                    }
                }
                Some(Moved::Stay)
            }
            Act::LogPanel => {
                let p = self.panels.panel();
                let values = p.values();
                if values.is_empty() {
                    return Some(Moved::Stay);
                }
                let profile = DatalogProfile {
                    name: p.name.clone(),
                    labels: logging::labels(&values),
                    values,
                    rate_hz: PANEL_HZ,
                };
                let name = profile.name.clone();
                client.datalog(DatalogAction::Save {
                    profile,
                    replaces: None,
                });
                client.datalog(DatalogAction::Start { profile: name });
                if self.panel == PanelScreen::Menu {
                    return self.data_back();
                }
                Some(Moved::Stay)
            }
            Act::Log(a) => {
                let st = client.snapshot().datalog.clone();
                if *a == logging::LogAct::Mark
                    && let Some(r) = st.as_ref().and_then(|d| d.running.as_ref())
                {
                    self.mem.borrow_mut().marks.push((r.marks + 1, Instant::now()));
                }
                Some(match self.log.apply(a, st.as_ref(), client) {
                    logging::Done::Stay => Moved::Stay,
                    logging::Done::Opened => {
                        self.trail.push(focus);
                        Moved::Opened
                    }
                    logging::Done::Back => self.data_back().unwrap_or(Moved::Stay),
                })
            }
            Act::Panel(a) => Some(self.panel_run(a, focus, client)),
            Act::ClearAll => {
                if !self.clear_all {
                    self.clear_all = true;
                    return Some(Moved::Stay);
                }
                self.clear_all = false;
                let snap = client.snapshot().clone();
                self.clearing = snap
                    .diag
                    .dtcs
                    .iter()
                    .filter(|(_, c)| !c.is_empty())
                    .map(|(m, _)| *m)
                    .rev()
                    .collect();
                self.tick(client);
                Some(Moved::Stay)
            }
            Act::Module(i) => {
                self.clear_all = false;
                self.module = Some(*i);
                self.trail.push(focus);
                let snap = client.snapshot().clone();
                if let Some(m) = snap.diag.modules().get(*i)
                    && !snap.diag.identity.contains_key(&m.module.tp2)
                    && !snap.diag.busy()
                    && snap.connected
                {
                    client.diag(DiagAction::Identify {
                        module: m.module.tp2,
                    });
                }
                Some(Moved::Opened)
            }
            _ => {
                self.clear_all = false;
                None
            }
        }
    }

    fn open_panel(&mut self, to: PanelScreen, focus: i32) -> Moved {
        self.panel = to;
        self.dropping = false;
        self.trail.push(focus);
        Moved::Opened
    }

    fn panel_run(&mut self, a: &PanelAct, focus: i32, client: &Client) -> Moved {
        use PanelAct as A;
        if !matches!(a, A::Remove(_) | A::RemovePanel(_)) {
            self.dropping = false;
        }
        let snap = client.snapshot().clone();
        let now = Instant::now();
        let moved = match a {
            A::Step(d) => {
                self.panels.step(*d);
                Moved::Stay
            }
            A::Menu => return self.open_panel(PanelScreen::Menu, focus),
            A::Edit => {
                // From the menu: the editor replaces it.
                if self.panel == PanelScreen::Menu {
                    self.panel = PanelScreen::Edit;
                    return Moved::Opened;
                }
                return self.open_panel(PanelScreen::Edit, focus);
            }
            A::List => {
                if self.panel == PanelScreen::Menu {
                    self.panel = PanelScreen::List;
                    return Moved::Opened;
                }
                return self.open_panel(PanelScreen::List, focus);
            }
            A::Add => {
                self.panels.shown = self.panels.scratch();
                self.pick = Default::default();
                if self.panel == PanelScreen::Menu {
                    self.panel = PanelScreen::Pick(Target::Scratch);
                    return Moved::Opened;
                }
                return self.open_panel(PanelScreen::Pick(Target::Scratch), focus);
            }
            A::SaveScratch => {
                let Some(name) = self.panels.free_name() else {
                    return Moved::Stay;
                };
                let i = self.panels.scratch();
                let mut p = self.panels.panels[i].clone();
                p.scratch = false;
                p.name = name.into();
                self.panels.panels[i].widgets.clear();
                self.panels.panels[i].relayout();
                self.panels.panels.insert(i, p);
                self.panels.shown = i;
                self.store();
                return self.data_back().unwrap_or(Moved::Stay);
            }
            A::ResetPeaks => {
                self.mem.borrow_mut().reset_peaks();
                return self.data_back().unwrap_or(Moved::Stay);
            }
            A::New => {
                let Some(name) = self.panels.free_name() else {
                    return Moved::Stay;
                };
                let at = self.panels.scratch();
                self.panels
                    .panels
                    .insert(at, Panel::new(name, Grid::G3x2, Vec::new()));
                self.panels.shown = at;
                self.store();
                self.panel = PanelScreen::Edit;
                self.trail.clear();
                self.trail.push(0);
                return Moved::Opened;
            }
            A::Grid(d) => {
                let p = self.panels.panel_mut();
                p.grid = cycle(&Grid::ALL, p.grid, *d);
                p.clamp();
                Moved::Stay
            }
            A::Widget(i) => return self.open_panel(PanelScreen::Widget(*i), focus),
            A::Cell(c, r) => {
                self.pick = Default::default();
                return self.open_panel(PanelScreen::Pick(Target::Cell(*c, *r)), focus);
            }
            A::Move(i, d) => {
                if panels::shift(self.panels.panel_mut(), *i, *d) {
                    // Focus follows the widget: parts are in widget order, so it stays put.
                    Moved::Stay
                } else {
                    return Moved::Stay;
                }
            }
            A::Value(w, k) => {
                self.pick = Default::default();
                return self.open_panel(PanelScreen::Pick(Target::Value(*w, *k)), focus);
            }
            A::Drop(w, k) => {
                if let Some(x) = self.panels.panel_mut().widgets.get_mut(*w)
                    && x.values.len() > 1
                    && *k < x.values.len()
                {
                    x.values.remove(*k);
                }
                Moved::Stay
            }
            A::Style(i, d) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i) {
                    let s = cycle(&Style::ALL, w.style, *d);
                    w.restyle(s);
                }
                Moved::Stay
            }
            A::Size(i, d) => {
                let p = self.panels.panel_mut();
                let all = panels::sizes(p, *i);
                if let Some(w) = p.widgets.get_mut(*i)
                    && !all.is_empty()
                {
                    let (ww, hh) = cycle(&all, (w.w, w.h), *d);
                    w.w = ww;
                    w.h = hh;
                }
                Moved::Stay
            }
            A::Decimals(i, d) => {
                self.with_widget(*i, &snap, now, |w, dp, _| {
                    w.opts.decimals = Some((i32::from(dp) + d).clamp(0, 3) as u8);
                });
                Moved::Stay
            }
            A::Unit(i, d) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i)
                    && let Some(first) = w.values.first()
                {
                    let base = defs::describe(first).unit;
                    let all = units_for(base);
                    if !all.is_empty() {
                        let now_u = w.opts.unit.clone().unwrap_or_else(|| base.to_string());
                        let next = cycle(all, all.iter().copied().find(|u| *u == now_u).unwrap_or(base), *d);
                        w.opts.unit = (next != base).then(|| next.to_string());
                        w.opts.decimals = None;
                        w.opts.range = None;
                        w.opts.warn = None;
                        w.opts.limit = None;
                    }
                }
                Moved::Stay
            }
            A::Relative(i) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i) {
                    w.opts.relative = !w.opts.relative;
                    w.opts.range = None;
                    w.opts.warn = None;
                    w.opts.limit = None;
                }
                Moved::Stay
            }
            A::MinMax(i, d) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i) {
                    w.opts.minmax = cycle(
                        &[MinMax::Off, MinMax::SinceStart, MinMax::Last30s],
                        w.opts.minmax,
                        *d,
                    );
                }
                Moved::Stay
            }
            A::Peak(i, d) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i) {
                    w.opts.peak = cycle(&[Peak::Off, Peak::Hold, Peak::Hold10s], w.opts.peak, *d);
                }
                Moved::Stay
            }
            A::Window(i, d) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i) {
                    let now_w = w.opts.window.unwrap_or(30);
                    w.opts.window = Some(cycle(&WINDOWS, now_w, *d));
                }
                Moved::Stay
            }
            A::Fixed(i) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i) {
                    w.opts.fixed = !w.opts.fixed;
                }
                Moved::Stay
            }
            A::DiffPct(i) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i) {
                    w.opts.diff_pct = !w.opts.diff_pct;
                }
                Moved::Stay
            }
            A::Segments(i, d) => {
                if let Some(w) = self.panels.panel_mut().widgets.get_mut(*i) {
                    let n = i32::from(w.opts.segments.unwrap_or(10)) + 2 * d;
                    w.opts.segments = Some(n.clamp(4, 20) as u8);
                }
                Moved::Stay
            }
            A::Low(i, d) | A::High(i, d) => {
                let high = matches!(a, A::High(..));
                self.with_widget(*i, &snap, now, |w, _, s| {
                    let (mut lo, mut hi) = w.opts.range.unwrap_or((s.min, s.max));
                    if high {
                        hi = (hi + f64::from(*d) * s.major).max(lo + s.major);
                    } else {
                        lo = (lo + f64::from(*d) * s.major).min(hi - s.major);
                    }
                    w.opts.range = Some((lo, hi));
                });
                Moved::Stay
            }
            A::Warn(i, d) | A::Limit(i, d) => {
                let limit = matches!(a, A::Limit(..));
                self.with_widget(*i, &snap, now, |w, _, s| {
                    let by = s.major / 4.0;
                    let from = if limit { s.limit } else { s.warn };
                    let x = from.unwrap_or((s.min + s.max) / 2.0) + f64::from(*d) * by;
                    let x = x.clamp(s.min, s.max);
                    if limit {
                        w.opts.limit = Some(x);
                    } else {
                        w.opts.warn = Some(x);
                    }
                });
                Moved::Stay
            }
            A::Remove(i) => {
                if !self.dropping {
                    self.dropping = true;
                    return Moved::Stay;
                }
                self.dropping = false;
                let p = self.panels.panel_mut();
                if *i < p.widgets.len() {
                    p.widgets.remove(*i);
                }
                if p.scratch {
                    p.relayout();
                }
                self.store();
                return self.data_back().unwrap_or(Moved::Stay);
            }
            A::Open(i) => return self.open_panel(PanelScreen::Settings(*i), focus),
            A::Reorder(i, d) => {
                let to = *i as i32 + d;
                let last = self.panels.scratch() as i32 - 1;
                if (0..=last).contains(&to) {
                    self.panels.panels.swap(*i, to as usize);
                    if self.panels.shown == *i {
                        self.panels.shown = to as usize;
                    } else if self.panels.shown == to as usize {
                        self.panels.shown = *i;
                    }
                    self.store();
                    // Focus goes with the panel: one row per panel, in order.
                    return Moved::Focus(focus + d);
                }
                return Moved::Stay;
            }
            A::Name(i, d) => {
                let taken: Vec<String> = self
                    .panels
                    .panels
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| j != i)
                    .map(|(_, p)| p.name.clone())
                    .collect();
                if let Some(p) = self.panels.panels.get_mut(*i) {
                    let all = panels::NAMES;
                    let len = all.len() as i32;
                    let from = all.iter().position(|n| *n == p.name).map_or(if *d > 0 { -1 } else { 0 }, |x| x as i32);
                    for k in 1..=len {
                        let next = all[(from + d * k).rem_euclid(len) as usize];
                        if !taken.iter().any(|t| t == next) {
                            p.name = next.into();
                            break;
                        }
                    }
                }
                Moved::Stay
            }
            A::PanelGrid(i, d) => {
                if let Some(p) = self.panels.panels.get_mut(*i) {
                    p.grid = cycle(&Grid::ALL, p.grid, *d);
                    p.clamp();
                }
                Moved::Stay
            }
            A::RemovePanel(i) => {
                if !self.dropping {
                    self.dropping = true;
                    return Moved::Stay;
                }
                self.dropping = false;
                if *i < self.panels.scratch() {
                    self.panels.panels.remove(*i);
                    self.panels.shown = self.panels.shown.min(self.panels.panels.len() - 1);
                }
                self.store();
                return self.data_back().unwrap_or(Moved::Stay);
            }
            A::Show(i) => {
                self.panels.shown = (*i).min(self.panels.panels.len() - 1);
                self.store();
                self.panel = PanelScreen::View;
                self.trail.clear();
                return Moved::Opened;
            }
        };
        self.store();
        moved
    }

    /// Runs `f` on widget `i` with its first value's decimals and scale.
    fn with_widget(
        &mut self,
        i: usize,
        snap: &Snapshot,
        now: Instant,
        f: impl FnOnce(&mut Widget, u8, panels::Scale),
    ) {
        let Some(w) = self.panels.panel().widgets.get(i).cloned() else {
            return;
        };
        let Some(first) = w.values.first() else {
            return;
        };
        let s = panels::scale_of(&w, first, snap, &self.mem.borrow(), now);
        let d = panels::draw(&w, snap, &self.mem.borrow(), now);
        let dp = d.value.split('.').nth(1).map_or(0, |x| x.len() as u8);
        if let Some(w) = self.panels.panel_mut().widgets.get_mut(i) {
            f(w, dp, s);
        }
    }

    /// The value picker's acts on the Panels tab.
    pub(super) fn panel_slots(&mut self, a: &SlotAct) -> Moved {
        let PanelScreen::Pick(target) = self.panel else {
            return Moved::Stay;
        };
        let names = match self.pick.apply(a, &slots::DATA, &self.saved.recent) {
            Picked::Moved => return Moved::Opened,
            Picked::Stay => return Moved::Stay,
            Picked::Chose(name) => name.into_iter().collect::<Vec<_>>(),
            Picked::ChoseAll(names) => names,
        };
        for n in &names {
            slots::remember(&mut self.saved.recent, n);
        }
        let mut rest = names.clone().into_iter().skip(1);
        let name = names.into_iter().next();
        // More than one (a group's fields): the rest go where they fit.
        match target {
            Target::Cell(c0, r0) => {
                for n in rest.by_ref() {
                    let p = self.panels.panel_mut();
                    let Some((c, r)) = p.free().into_iter().find(|&at| at != (c0, r0)) else {
                        break;
                    };
                    p.widgets.push(Widget::new(c, r, Style::Number, &[&n]));
                }
            }
            Target::Value(w, _) => {
                if let Some(x) = self.panels.panel_mut().widgets.get_mut(w) {
                    let (_, most) = x.style.takes();
                    for n in rest.by_ref() {
                        if x.values.len() < most && !x.values.contains(&n) {
                            x.values.push(n);
                        }
                    }
                }
            }
            Target::Scratch => {
                let i = self.panels.scratch();
                for n in rest.by_ref() {
                    self.panels.panels[i]
                        .widgets
                        .push(Widget::new(0, 0, Style::Number, &[&n]));
                }
            }
        }
        match (target, name) {
            (Target::Cell(c, r), Some(n)) => {
                let style = if n == "trans.selector" {
                    Style::Selector
                } else {
                    Style::Number
                };
                self.panels
                    .panel_mut()
                    .widgets
                    .push(Widget::new(c, r, style, &[&n]));
            }
            (Target::Value(w, k), name) => {
                if let Some(x) = self.panels.panel_mut().widgets.get_mut(w) {
                    match name {
                        Some(n) if k < x.values.len() => x.values[k] = n,
                        Some(n) => x.values.push(n),
                        None if x.values.len() > 1 && k < x.values.len() => {
                            x.values.remove(k);
                        }
                        None => {}
                    }
                }
            }
            (Target::Scratch, Some(n)) => {
                let i = self.panels.scratch();
                let p = &mut self.panels.panels[i];
                p.widgets.push(Widget::new(0, 0, Style::Number, &[&n]));
                p.relayout();
                self.panels.shown = i;
            }
            _ => {}
        }
        self.store();
        self.pick = Default::default();
        self.panel = self.panel.parent().unwrap_or_default();
        Moved::Closed(self.trail.pop().unwrap_or(0))
    }

    pub(super) fn data_view(&self, snap: &Snapshot, now: Instant) -> View {
        let mut v = self.data_screen(snap, now);
        // A running log's timer on every Data screen.
        let t = rec(snap);
        if !t.is_empty() && v.hint.is_empty() {
            v.hint = format!("● {t}");
        }
        v
    }

    fn data_screen(&self, snap: &Snapshot, now: Instant) -> View {
        match self.tab {
            DataTab::Panels => self.panels_view(snap, now),
            DataTab::Logs => match self.log.screen {
                LogScreen::Top => {
                    let p = self.panels.panel();
                    let has = !p.widgets.is_empty();
                    logging::top(
                        tabbed("Data", DataTab::Logs),
                        &self.log,
                        snap,
                        has.then_some(p.name.as_str()),
                    )
                }
                LogScreen::Profiles => logging::profiles(snap),
                LogScreen::Profile => logging::profile(&self.log, snap),
                LogScreen::Channels => logging::channels(&self.log, self.recent(), snap, now),
            },
            DataTab::Faults => match self.module {
                Some(i) => module(snap, now, i, self.confirm),
                None => modules(
                    tabbed("Data", DataTab::Faults),
                    snap,
                    self.clear_all,
                    self.clearing.len(),
                ),
            },
        }
    }

    fn panels_view(&self, snap: &Snapshot, now: Instant) -> View {
        let p = self.panels.panel();
        self.mem
            .borrow_mut()
            .sample(&panels::sampled(p), snap, now);
        let mem = self.mem.borrow();
        let mut v = match self.panel {
            PanelScreen::View => {
                let mut v = View::new(Layout::Panel, p.name.clone(), false);
                tabs(&mut v, DataTab::Panels);
                // Empty Scratch: push goes straight to the picker.
                let push = if p.scratch && p.widgets.is_empty() {
                    PanelAct::Add
                } else {
                    PanelAct::Menu
                };
                let at = v.panel_part(pa(push), sides(PanelAct::Step(-1), PanelAct::Step(1)));
                v.flip = at;
                v.start = at;
                v.side = sides(PanelAct::Step(-1), PanelAct::Step(1));
                let mut d = panels::view(p, snap, &mem, now);
                d.dots = self.panels.panels.len();
                d.at = self.panels.shown;
                v.panel = Some(d);
                v
            }
            PanelScreen::Menu => self.menu(snap),
            PanelScreen::Edit => self.editor(snap, now),
            PanelScreen::Widget(i) => self.widget(i, snap, now),
            PanelScreen::Pick(t) => {
                let current = match t {
                    Target::Value(w, k) => p.widgets.get(w).and_then(|x| x.values.get(k)).cloned(),
                    _ => None,
                };
                let title = match t {
                    Target::Value(w, k) if p.widgets.get(w).is_some_and(|x| k < x.values.len()) => "Value",
                    _ => "Add value",
                };
                self.pick
                    .view(title, current.as_deref(), self.recent(), snap, now, &slots::DATA)
            }
            PanelScreen::List => self.list(),
            PanelScreen::Settings(i) => self.settings(i),
        };
        if !snap.connected && v.note.is_empty() {
            v.note = problem(snap);
        }
        v.finish()
    }

    fn menu(&self, snap: &Snapshot) -> View {
        let p = self.panels.panel();
        let mut v = View::new(Layout::List, p.name.clone(), true);
        let free = self.panels.free_name().is_some();
        if p.scratch {
            v.row(Row::new(RowKind::Button, "Add value", "").button("Add").act(pa(PanelAct::Add)));
            if !p.widgets.is_empty() && free {
                v.row(
                    Row::new(RowKind::Button, "Save as panel", "")
                        .button("Save")
                        .act(pa(PanelAct::SaveScratch)),
                );
            }
        } else {
            v.row(Row::new(RowKind::Route, "Edit panel", "").act(pa(PanelAct::Edit)));
        }
        let running = snap.datalog.as_ref().is_some_and(|d| d.running.is_some());
        if !p.widgets.is_empty() && snap.connected && !running {
            v.row(Row::new(RowKind::Button, "Log this panel", "").button("Start").act(Act::LogPanel));
        }
        v.row(Row::new(RowKind::Button, "Reset peaks", "").button("Reset").act(pa(PanelAct::ResetPeaks)));
        if free {
            v.row(Row::new(RowKind::Button, "New panel", "").button("New").act(pa(PanelAct::New)));
        }
        v.row(Row::new(RowKind::Route, "Panels", "").act(pa(PanelAct::List)));
        v
    }

    fn editor(&self, snap: &Snapshot, now: Instant) -> View {
        let p = self.panels.panel();
        let mut v = View::new(Layout::Panel, format!("Edit · {}", p.name), true);
        v.pill(format!("Grid {}", p.grid.label()), false, pa(PanelAct::Grid(1))).side =
            sides(PanelAct::Grid(-1), PanelAct::Grid(1));
        let mem = self.mem.borrow();
        let mut d = panels::view(p, snap, &mem, now);
        d.edit = true;
        d.hint.clear();
        let mut first = -1;
        for (i, w) in d.widgets.iter_mut().enumerate() {
            let at = v.panel_part(
                pa(PanelAct::Widget(i)),
                sides(PanelAct::Move(i, -1), PanelAct::Move(i, 1)),
            );
            w.focus_at = at;
            if first < 0 {
                first = at;
            }
        }
        for (c, r) in p.free() {
            let at = v.panel_part(pa(PanelAct::Cell(c, r)), None);
            d.empties.push(Empty {
                col: c,
                row: r,
                focus_at: at,
            });
            if first < 0 {
                first = at;
            }
        }
        v.start = first;
        v.panel = Some(d);
        v
    }

    fn widget(&self, i: usize, snap: &Snapshot, now: Instant) -> View {
        let p = self.panels.panel();
        let Some(w) = p.widgets.get(i) else {
            return View::new(Layout::List, "Widget", true);
        };
        let mem = self.mem.borrow();
        let mut d = panels::view(p, snap, &mem, now);
        d.edit = true;
        d.hint.clear();
        if let Some(x) = d.widgets.get_mut(i) {
            x.picked = true;
        }
        let label = d.widgets.get(i).map_or_else(String::new, |x| x.label.clone());
        let mut v = View::new(Layout::List, label, true);
        v.panel = Some(d);

        let (_, most) = w.style.takes();
        if most == 1 {
            let name = w.values.first().map(|n| crate::slots::label(n)).unwrap_or_default();
            v.row(Row::new(RowKind::Route, "Value", name).act(pa(PanelAct::Value(i, 0))));
        } else {
            for (k, n) in w.values.iter().enumerate() {
                v.row(
                    Row::new(RowKind::Route, format!("Value {}", k + 1), crate::slots::label(n))
                        .act(pa(PanelAct::Value(i, k))),
                );
            }
            if w.values.len() < most {
                v.row(
                    Row::new(RowKind::Button, "Add value", "")
                        .button("Add")
                        .act(pa(PanelAct::Value(i, w.values.len()))),
                );
            }
        }
        v.row(choice("Style", w.style.label(), |d| PanelAct::Style(i, d)));
        let size = format!("{}×{}", w.w, w.h);
        v.row(choice("Size", size, |d| PanelAct::Size(i, d)));

        let first = w.values.first().map(String::as_str).unwrap_or("");
        let base = defs::describe(first).unit;
        let numeric = matches!(defs::describe(first).kind, defs::Kind::Num(_));
        let s = panels::scale_of(w, first, snap, &mem, now);
        let fmt = |x: f64| crate::values::number(x, if s.major < 1.0 { 2 } else { 0 });
        if numeric && w.style != Style::Selector {
            if absolute_pressure(first) {
                let rel = if w.opts.relative { "Relative" } else { "Absolute" };
                v.row(choice("Pressure", rel, |_| PanelAct::Relative(i)));
            }
            if !units_for(base).is_empty() {
                let u = w.opts.unit.clone().unwrap_or_else(|| base.to_string());
                v.row(choice("Unit", u, |d| PanelAct::Unit(i, d)));
            }
            let dp = d_dp(&v, i);
            v.row(step("Decimals", dp.to_string(), |d| PanelAct::Decimals(i, d)));
        }
        match w.style {
            Style::Number => {
                let m = match w.opts.minmax {
                    MinMax::Off => "Off",
                    MinMax::SinceStart => "Since start",
                    MinMax::Last30s => "Last 30 s",
                };
                v.row(choice("Min/max", m, |d| PanelAct::MinMax(i, d)));
            }
            Style::Gauge => {
                let pk = match w.opts.peak {
                    Peak::Off => "Off",
                    Peak::Hold => "Hold",
                    Peak::Hold10s => "Hold 10 s",
                };
                v.row(choice("Peak", pk, |d| PanelAct::Peak(i, d)));
                v.row(step("Low", fmt(s.min), |d| PanelAct::Low(i, d)));
                v.row(step("High", fmt(s.max), |d| PanelAct::High(i, d)));
                let red = s.limit.or(s.warn).map_or("Off".to_string(), fmt);
                v.row(step("Red from", red, |d| PanelAct::Limit(i, d)));
            }
            Style::Bar => {
                v.row(step("Low", fmt(s.min), |d| PanelAct::Low(i, d)));
                v.row(step("High", fmt(s.max), |d| PanelAct::High(i, d)));
                v.row(step("Warn from", s.warn.map_or("Off".into(), fmt), |d| PanelAct::Warn(i, d)));
                v.row(step("Limit from", s.limit.map_or("Off".into(), fmt), |d| PanelAct::Limit(i, d)));
                let n = w.opts.segments.unwrap_or(10);
                v.row(step("Segments", n.to_string(), |d| PanelAct::Segments(i, d)));
            }
            Style::Graph => {
                let win = format!("{} s", w.opts.window.unwrap_or(30));
                v.row(choice("Window", win, |d| PanelAct::Window(i, d)));
                let r = if w.opts.fixed { "Fixed" } else { "Auto" };
                v.row(choice("Range", r, |_| PanelAct::Fixed(i)));
                if w.opts.fixed {
                    v.row(step("Low", fmt(s.min), |d| PanelAct::Low(i, d)));
                    v.row(step("High", fmt(s.max), |d| PanelAct::High(i, d)));
                }
            }
            Style::Pair => {
                let r = if w.opts.diff_pct { "%" } else { "Absolute" };
                v.row(choice("Difference", r, |_| PanelAct::DiffPct(i)));
            }
            Style::Set | Style::Selector => {}
        }
        v.section("");
        let mut rm = Row::new(
            RowKind::Button,
            if self.dropping { "Confirm remove" } else { "Remove" },
            "",
        )
        .button("Remove")
        .act(pa(PanelAct::Remove(i)));
        rm.alert = self.dropping;
        v.row(rm);
        v
    }

    fn list(&self) -> View {
        let mut v = View::new(Layout::List, "Panels", true);
        for (i, p) in self.panels.panels.iter().enumerate() {
            if p.scratch {
                continue;
            }
            let row = Row::new(RowKind::Route, p.name.clone(), "")
                .act(pa(PanelAct::Open(i)))
                .side(pa(PanelAct::Reorder(i, -1)), pa(PanelAct::Reorder(i, 1)));
            let at = v.row(row);
            if i == self.panels.shown {
                v.start = at;
            }
        }
        if self.panels.free_name().is_some() {
            v.row(Row::new(RowKind::Button, "New panel", "").button("New").act(pa(PanelAct::New)));
        }
        v
    }

    fn settings(&self, i: usize) -> View {
        let Some(p) = self.panels.panels.get(i) else {
            return View::new(Layout::List, "Panel", true);
        };
        let mut v = View::new(Layout::List, p.name.clone(), true);
        v.row(choice("Name", p.name.clone(), |d| PanelAct::Name(i, d)));
        v.row(choice("Grid", p.grid.label(), |d| PanelAct::PanelGrid(i, d)));
        v.row(Row::new(RowKind::Button, "Show", "").button("Show").act(pa(PanelAct::Show(i))));
        v.section("");
        let mut rm = Row::new(
            RowKind::Button,
            if self.dropping { "Confirm remove" } else { "Remove" },
            "",
        )
        .button("Remove")
        .act(pa(PanelAct::RemovePanel(i)));
        rm.alert = self.dropping;
        v.row(rm);
        v
    }
}

/// The decimals widget `i` shows now, from the drawn value.
fn d_dp(v: &View, i: usize) -> u8 {
    v.panel
        .as_ref()
        .and_then(|d| d.widgets.get(i))
        .and_then(|w| w.value.split('.').nth(1))
        .map_or(0, |x| x.len() as u8)
}
