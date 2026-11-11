// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! Data panels (docs/PAGES.md 5c): pages of widgets on a small grid, and how each widget draws.
//!
//! A panel is a grid (1×1 up to 4×4) and widgets that cover one or more of its cells. A widget
//! is a style (number, gauge, bar, graph, pair, set, selector), its values and a few options;
//! anything left unset comes from [`scale`], the catalog's range and limits for the value.
//! Drawing is plain data ([`PanelDraw`]): Slint only places it. Gauge ticks and graph lines are
//! SVG path commands in a 1000×1000 box.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use carpi_proto::Value;
use serde::{Deserialize, Serialize};

use crate::client::Snapshot;
use crate::defs::{self, Kind};
use crate::values::{self, Freshness, NONE};

/// How much history a graph keeps.
pub const HISTORY: Duration = Duration::from_secs(62);
/// The atmosphere when nothing says otherwise.
const ATM_MBAR: f64 = 1013.25;

// ---------------------------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------------------------

/// Columns × rows a panel is laid out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Grid {
    #[serde(rename = "1x1")]
    G1x1,
    #[serde(rename = "2x1")]
    G2x1,
    #[serde(rename = "2x2")]
    G2x2,
    #[serde(rename = "3x2")]
    #[default]
    G3x2,
    #[serde(rename = "3x3")]
    G3x3,
    #[serde(rename = "4x3")]
    G4x3,
    #[serde(rename = "4x4")]
    G4x4,
}

impl Grid {
    pub const ALL: [Grid; 7] = [
        Grid::G1x1,
        Grid::G2x1,
        Grid::G2x2,
        Grid::G3x2,
        Grid::G3x3,
        Grid::G4x3,
        Grid::G4x4,
    ];

    /// (columns, rows).
    pub fn size(self) -> (u8, u8) {
        match self {
            Grid::G1x1 => (1, 1),
            Grid::G2x1 => (2, 1),
            Grid::G2x2 => (2, 2),
            Grid::G3x2 => (3, 2),
            Grid::G3x3 => (3, 3),
            Grid::G4x3 => (4, 3),
            Grid::G4x4 => (4, 4),
        }
    }

    pub fn label(self) -> String {
        let (c, r) = self.size();
        format!("{c}×{r}")
    }

    /// The smallest grid that holds `n` one-cell widgets: Scratch's auto layout.
    pub fn fit(n: usize) -> Grid {
        match n {
            0 | 1 => Grid::G1x1,
            2 => Grid::G2x1,
            3 | 4 => Grid::G2x2,
            5 | 6 => Grid::G3x2,
            7..=9 => Grid::G3x3,
            10..=12 => Grid::G4x3,
            _ => Grid::G4x4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    #[default]
    Number,
    Gauge,
    Bar,
    Graph,
    Pair,
    Set,
    Selector,
}

impl Style {
    pub const ALL: [Style; 7] = [
        Style::Number,
        Style::Gauge,
        Style::Bar,
        Style::Graph,
        Style::Pair,
        Style::Set,
        Style::Selector,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Style::Number => "Number",
            Style::Gauge => "Gauge",
            Style::Bar => "Bar",
            Style::Graph => "Graph",
            Style::Pair => "Pair",
            Style::Set => "Set",
            Style::Selector => "Selector",
        }
    }

    /// How many values the style takes: at least, at most.
    pub fn takes(self) -> (usize, usize) {
        match self {
            Style::Graph => (1, 3),
            Style::Pair => (2, 2),
            Style::Set => (1, 4),
            _ => (1, 1),
        }
    }

    /// A number the D-pad steps with.
    fn index(self) -> usize {
        Style::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The code Slint draws by.
    pub fn code(self) -> i32 {
        self.index() as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MinMax {
    #[default]
    Off,
    SinceStart,
    Last30s,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Peak {
    Off,
    #[default]
    Hold,
    Hold10s,
}

/// A widget's options. `None` is the catalog's default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Opts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    /// Shown unit where the value converts: `bar`, `psi`, `°F`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Pressures less the atmosphere: boost, below zero vacuum.
    pub relative: bool,
    pub minmax: MinMax,
    pub peak: Peak,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<(f64, f64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments: Option<u8>,
    /// Graph window, seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<u16>,
    /// Graph range from `range` rather than what's in the window.
    pub fixed: bool,
    /// Pair: the difference in % rather than units.
    pub diff_pct: bool,
}

/// One widget: where it sits, what it shows and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Widget {
    pub col: u8,
    pub row: u8,
    #[serde(default = "one")]
    pub w: u8,
    #[serde(default = "one")]
    pub h: u8,
    #[serde(default)]
    pub style: Style,
    pub values: Vec<String>,
    #[serde(default)]
    pub opts: Opts,
}

fn one() -> u8 {
    1
}

impl Widget {
    pub fn new(col: u8, row: u8, style: Style, values: &[&str]) -> Self {
        Self {
            col,
            row,
            w: 1,
            h: 1,
            style,
            values: values.iter().map(|v| v.to_string()).collect(),
            opts: Opts::default(),
        }
    }

    pub fn size(mut self, w: u8, h: u8) -> Self {
        self.w = w;
        self.h = h;
        self
    }

    pub fn covers(&self, col: u8, row: u8) -> bool {
        col >= self.col && col < self.col + self.w && row >= self.row && row < self.row + self.h
    }

    /// Changes the style; options that no longer apply go back to the catalog's.
    pub fn restyle(&mut self, style: Style) {
        let o = &mut self.opts;
        let keeps_range = matches!(style, Style::Gauge | Style::Bar | Style::Graph);
        if !keeps_range {
            o.range = None;
            o.warn = None;
            o.limit = None;
            o.fixed = false;
        }
        if style != Style::Bar {
            o.segments = None;
        }
        if style != Style::Graph {
            o.window = None;
        }
        if style != Style::Number {
            o.minmax = MinMax::Off;
        }
        if style != Style::Pair {
            o.diff_pct = false;
        }
        let (_, most) = style.takes();
        self.values.truncate(most);
        self.style = style;
    }
}

/// A page of widgets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub name: String,
    #[serde(default)]
    pub grid: Grid,
    #[serde(default)]
    pub widgets: Vec<Widget>,
    /// Lays itself out as values are added (the last panel).
    #[serde(default)]
    pub scratch: bool,
}

impl Panel {
    pub fn new(name: &str, grid: Grid, widgets: Vec<Widget>) -> Self {
        Self {
            name: name.into(),
            grid,
            widgets,
            scratch: false,
        }
    }

    pub fn scratch() -> Self {
        Self {
            name: "Scratch".into(),
            grid: Grid::G1x1,
            widgets: Vec::new(),
            scratch: true,
        }
    }

    /// The widget covering a cell.
    pub fn at(&self, col: u8, row: u8) -> Option<usize> {
        self.widgets.iter().position(|w| w.covers(col, row))
    }

    /// Cells nothing covers, in reading order.
    pub fn free(&self) -> Vec<(u8, u8)> {
        let (cols, rows) = self.grid.size();
        let mut out = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                if self.at(c, r).is_none() {
                    out.push((c, r));
                }
            }
        }
        out
    }

    /// Every value its widgets show.
    pub fn values(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for w in &self.widgets {
            for v in &w.values {
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
        }
        out
    }

    /// Drops widgets that no longer fit the grid, and shrinks ones that overhang.
    pub fn clamp(&mut self) {
        let (cols, rows) = self.grid.size();
        self.widgets.retain(|w| w.col < cols && w.row < rows);
        for w in &mut self.widgets {
            w.w = w.w.clamp(1, cols - w.col);
            w.h = w.h.clamp(1, rows - w.row);
        }
        // Later widgets lose to earlier ones where they overlap.
        let mut kept: Vec<Widget> = Vec::new();
        for w in self.widgets.drain(..) {
            let clash = kept.iter().any(|k| {
                w.col < k.col + k.w && k.col < w.col + w.w && w.row < k.row + k.h && k.row < w.row + w.h
            });
            if !clash {
                kept.push(w);
            }
        }
        self.widgets = kept;
    }

    /// Scratch: one cell each, in order, on the smallest grid that holds them.
    pub fn relayout(&mut self) {
        self.grid = Grid::fit(self.widgets.len().max(1));
        let (cols, _) = self.grid.size();
        for (i, w) in self.widgets.iter_mut().enumerate() {
            w.col = (i % cols as usize) as u8;
            w.row = (i / cols as usize) as u8;
            w.w = 1;
            w.h = 1;
        }
        self.clamp();
    }

    /// Room for a `w`×`h` widget at (`col`, `row`).
    pub fn fits(&self, skip: Option<usize>, col: u8, row: u8, w: u8, h: u8) -> bool {
        let (cols, rows) = self.grid.size();
        if col + w > cols || row + h > rows {
            return false;
        }
        self.widgets.iter().enumerate().all(|(i, k)| {
            Some(i) == skip
                || !(col < k.col + k.w && k.col < col + w && row < k.row + k.h && k.row < row + h)
        })
    }
}

/// The starting panels: today's groups as numbers, plus Scratch. Better ones come later.
pub fn defaults() -> Vec<Panel> {
    let mut out: Vec<Panel> = defs::PRESETS
        .iter()
        .map(|p| {
            let grid = Grid::fit(p.values.len());
            let (cols, _) = grid.size();
            let widgets = p
                .values
                .iter()
                .take(16)
                .enumerate()
                .map(|(i, v)| {
                    Widget::new(
                        (i % cols as usize) as u8,
                        (i / cols as usize) as u8,
                        Style::Number,
                        &[v],
                    )
                })
                .collect();
            Panel::new(p.name, grid, widgets)
        })
        .collect();
    out.push(Panel::scratch());
    out
}

/// Panels and which is shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Panels {
    pub panels: Vec<Panel>,
    #[serde(default)]
    pub shown: usize,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            panels: defaults(),
            shown: 0,
        }
    }
}

impl Panels {
    /// Scratch last, at least one panel, `shown` in range.
    pub fn fixed(mut self) -> Self {
        let scratch: Vec<Panel> = self.panels.iter().filter(|p| p.scratch).cloned().collect();
        self.panels.retain(|p| !p.scratch);
        self.panels
            .push(scratch.into_iter().next().unwrap_or_else(Panel::scratch));
        for p in &mut self.panels {
            p.clamp();
        }
        self.shown = self.shown.min(self.panels.len() - 1);
        self
    }

    pub fn panel(&self) -> &Panel {
        &self.panels[self.shown.min(self.panels.len() - 1)]
    }

    pub fn panel_mut(&mut self) -> &mut Panel {
        let i = self.shown.min(self.panels.len() - 1);
        &mut self.panels[i]
    }

    pub fn step(&mut self, d: i32) {
        let n = self.panels.len() as i32;
        self.shown = (self.shown as i32 + d).clamp(0, n - 1) as usize;
    }

    /// Scratch's index.
    pub fn scratch(&self) -> usize {
        self.panels.len() - 1
    }

    /// A name from [`NAMES`] no panel has.
    pub fn free_name(&self) -> Option<&'static str> {
        NAMES
            .iter()
            .copied()
            .find(|n| !self.panels.iter().any(|p| p.name == *n))
    }
}

/// Panel names on the head unit; the portal sets others.
pub const NAMES: &[&str] = &[
    "Daily", "Engine", "Boost", "Haldex", "Electrical", "Trip", "Custom 1", "Custom 2", "Custom 3",
    "Custom 4",
];

// ---------------------------------------------------------------------------------------------
// The catalog's ranges
// ---------------------------------------------------------------------------------------------

/// A value's range and limits, in its own unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub warn: Option<f64>,
    pub limit: Option<f64>,
    /// Labelled ticks every this much.
    pub major: f64,
}

const fn sc(min: f64, max: f64, warn: Option<f64>, limit: Option<f64>, major: f64) -> Scale {
    Scale {
        min,
        max,
        warn,
        limit,
        major,
    }
}

/// The range a value draws in. Pressures are absolute mbar.
pub fn scale(name: &str) -> Option<Scale> {
    Some(match name {
        "engine.rpm" => sc(0.0, 7000.0, Some(6000.0), Some(6500.0), 1000.0),
        "vehicle.speed_kmh" => sc(0.0, 260.0, None, None, 40.0),
        "engine.coolant_c" => sc(40.0, 130.0, Some(105.0), Some(115.0), 20.0),
        "engine.oil_temp_c" => sc(40.0, 150.0, Some(120.0), Some(135.0), 20.0),
        "engine.intake_temp_c" => sc(-20.0, 80.0, Some(50.0), Some(65.0), 20.0),
        "awd.oil_temp_c" | "awd.plate_temp_c" => sc(0.0, 150.0, Some(100.0), Some(120.0), 25.0),
        "engine.boost.actual"
        | "engine.boost.specified"
        | "engine.boost.cluster_mbar"
        | "engine.manifold_mbar" => sc(0.0, 2800.0, None, None, 500.0),
        "engine.atmospheric_mbar" => sc(800.0, 1100.0, None, None, 100.0),
        "engine.load_pct" => sc(0.0, 200.0, None, None, 50.0),
        "engine.throttle_pct" | "engine.pedal_pct" | "engine.n75_duty_pct" => {
            sc(0.0, 100.0, None, None, 25.0)
        }
        "engine.lambda.actual" | "engine.lambda.specified" => sc(0.6, 1.4, None, None, 0.2),
        "engine.fuel_rail.actual_bar" | "engine.fuel_rail.specified_bar" => {
            sc(0.0, 140.0, None, None, 20.0)
        }
        "engine.ignition_timing_deg" => sc(-10.0, 40.0, None, None, 10.0),
        "engine.maf_gs" => sc(0.0, 250.0, None, None, 50.0),
        n if n.starts_with("engine.timing_retard.") => sc(-12.0, 0.0, Some(-3.0), Some(-6.0), 3.0),
        "power.battery_v" | "engine.terminal30_v" | "awd.supply_v" => {
            sc(10.0, 16.0, None, None, 1.0)
        }
        "fuel.level_l" => sc(0.0, 55.0, None, None, 10.0),
        "awd.valve_opening_pct" => sc(0.0, 100.0, None, None, 25.0),
        "awd.estimated_torque_nm" => sc(0.0, 1600.0, None, None, 400.0),
        _ => return None,
    })
}

/// Pressures that are absolute and may be shown relative.
pub fn absolute_pressure(name: &str) -> bool {
    matches!(
        name,
        "engine.boost.actual"
            | "engine.boost.specified"
            | "engine.boost.cluster_mbar"
            | "engine.manifold_mbar"
    )
}

/// Units a value's own unit converts to.
pub fn units_for(unit: &str) -> &'static [&'static str] {
    match unit {
        "mbar" => &["mbar", "bar", "psi"],
        "bar" => &["bar", "psi"],
        "°C" => &["°C", "°F"],
        "km/h" => &["km/h", "mph"],
        _ => &[],
    }
}

/// `x` in `from` as `to`; affine for temperatures. Differences (`delta`) drop the offset.
pub fn convert(x: f64, from: &str, to: &str, delta: bool) -> f64 {
    let k = |a: &str| match a {
        "mbar" => 1.0,
        "bar" => 1000.0,
        "psi" => 68.947_573,
        "km/h" => 1.0,
        "mph" => 1.609_344,
        _ => 1.0,
    };
    match (from, to) {
        ("°C", "°F") => x * 9.0 / 5.0 + if delta { 0.0 } else { 32.0 },
        (a, b) if a == b => x,
        (a, b) if units_for(a).contains(&b) => x * k(a) / k(b),
        _ => x,
    }
}

/// Decimals that suit a unit.
fn unit_dp(unit: &str) -> Option<u8> {
    Some(match unit {
        "bar" => 2,
        "psi" => 1,
        "mbar" => 0,
        "°F" => 0,
        "mph" => 0,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// History, peaks, marks
// ---------------------------------------------------------------------------------------------

/// What the panels remember between frames: recent samples for graphs and min/max, peaks.
#[derive(Debug, Default)]
pub struct Memory {
    samples: BTreeMap<String, VecDeque<(Instant, f64)>>,
    /// Highest since reset, with when.
    peaks: BTreeMap<String, (f64, Instant)>,
    lows: BTreeMap<String, f64>,
    /// Log marks: number and when.
    pub marks: Vec<(u32, Instant)>,
}

impl Memory {
    /// Takes the current reading of each of `names`.
    pub fn sample(&mut self, names: &[String], snap: &Snapshot, now: Instant) {
        for n in names {
            let Some(x) = raw(snap, n, now) else { continue };
            let q = self.samples.entry(n.clone()).or_default();
            if q.back().is_none_or(|(t, _)| now.saturating_duration_since(*t) >= Duration::from_millis(80)) {
                q.push_back((now, x));
            }
            while q.front().is_some_and(|(t, _)| now.saturating_duration_since(*t) > HISTORY) {
                q.pop_front();
            }
            let p = self.peaks.entry(n.clone()).or_insert((x, now));
            if x >= p.0 {
                *p = (x, now);
            }
            let l = self.lows.entry(n.clone()).or_insert(x);
            if x < *l {
                *l = x;
            }
        }
        self.marks
            .retain(|(_, t)| now.saturating_duration_since(*t) <= HISTORY);
    }

    pub fn reset_peaks(&mut self) {
        self.peaks.clear();
        self.lows.clear();
    }

    fn peak(&self, name: &str, now: Instant, hold: Peak) -> Option<f64> {
        let (x, at) = *self.peaks.get(name)?;
        match hold {
            Peak::Off => None,
            Peak::Hold => Some(x),
            Peak::Hold10s if now.saturating_duration_since(at) <= Duration::from_secs(10) => Some(x),
            Peak::Hold10s => self.window_max(name, now, Duration::from_secs(10)),
        }
    }

    fn window_max(&self, name: &str, now: Instant, span: Duration) -> Option<f64> {
        self.samples
            .get(name)?
            .iter()
            .filter(|(t, _)| now.saturating_duration_since(*t) <= span)
            .map(|(_, x)| *x)
            .reduce(f64::max)
    }

    fn window_min(&self, name: &str, now: Instant, span: Duration) -> Option<f64> {
        self.samples
            .get(name)?
            .iter()
            .filter(|(t, _)| now.saturating_duration_since(*t) <= span)
            .map(|(_, x)| *x)
            .reduce(f64::min)
    }

    fn series(&self, name: &str) -> impl Iterator<Item = &(Instant, f64)> {
        self.samples.get(name).into_iter().flatten()
    }
}

/// A numeric reading, fresh or not; `None` when there's none.
fn raw(snap: &Snapshot, name: &str, _now: Instant) -> Option<f64> {
    match snap.values.get(name)?.value.as_ref()? {
        Value::Num(n) if n.is_finite() => Some(*n),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// The atmosphere now, mbar: ICAN's altitude factor (faster), else the engine's group 113.
pub fn atmosphere(snap: &Snapshot, now: Instant) -> f64 {
    let fresh = |n: &str| {
        let s = snap.values.get(n);
        (values::freshness(s, now) == Freshness::Fresh)
            .then(|| raw(snap, n, now))
            .flatten()
    };
    fresh("engine.altitude_factor")
        .map(|f| f * ATM_MBAR)
        .or_else(|| fresh("engine.atmospheric_mbar"))
        .or_else(|| raw(snap, "engine.atmospheric_mbar", now))
        .unwrap_or(ATM_MBAR)
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

/// A line of a graph, in a 1000×1000 box, y down.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Line {
    pub path: String,
    /// 0 teal (first value), 1 white dashed (specified), 2 amber.
    pub color: i32,
    pub label: String,
}

/// One part of a set or selector.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub label: String,
    pub value: String,
    /// 0..1, or below 0 for none.
    pub frac: f32,
    /// Lit: the current selector position, an open door.
    pub on: bool,
    /// 0 normal, 1 warn, 2 limit.
    pub state: i32,
}

/// A labelled tick of a gauge: where (0..1 of the box), and the number.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tick {
    pub x: f32,
    pub y: f32,
    pub label: String,
}

/// A widget ready to place.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WidgetDraw {
    pub col: u8,
    pub row: u8,
    pub w: u8,
    pub h: u8,
    pub style: i32,
    pub label: String,
    pub value: String,
    pub unit: String,
    /// A second line: peak, min/max, the difference.
    pub sub: String,
    /// The second line in teal.
    pub sub_hi: bool,
    /// Pair: the second value.
    pub value2: String,
    pub label2: String,
    /// 0 normal, 1 warn, 2 limit; greyed when `stale`.
    pub state: i32,
    pub stale: bool,
    /// 0..1 of the range.
    pub frac: f32,
    /// Gauge paths: unlit ticks, lit, red zone unlit, red zone lit, needle, peak.
    pub ticks_off: String,
    pub ticks_on: String,
    pub red_off: String,
    pub red_on: String,
    pub needle: String,
    pub peak: String,
    pub labels: Vec<Tick>,
    /// Bar: segments, how many lit, where amber and red start.
    pub segs: i32,
    pub lit: i32,
    pub warn_from: i32,
    pub limit_from: i32,
    pub lines: Vec<Line>,
    /// Log marks on a graph: x in 0..1 and the number.
    pub marks: Vec<Tick>,
    pub items: Vec<Item>,
    /// Edit: takes focus at this index, -1 never.
    pub focus_at: i32,
    /// Edit: the widget being changed.
    pub picked: bool,
}

/// A free cell in the editor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Empty {
    pub col: u8,
    pub row: u8,
    pub focus_at: i32,
}

/// A panel ready to place.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PanelDraw {
    pub cols: u8,
    pub rows: u8,
    pub widgets: Vec<WidgetDraw>,
    pub empties: Vec<Empty>,
    /// Page dots: how many and which.
    pub dots: usize,
    pub at: usize,
    /// The editor: outlines, empty cells.
    pub edit: bool,
    /// One line in an empty panel.
    pub hint: String,
}

/// A value as a widget shows it: number in the widget's unit, text, unit, state.
struct Read {
    x: Option<f64>,
    text: String,
    unit: String,
    label: String,
    stale: bool,
    dp: u8,
}

/// How `name` reads in widget `w`.
fn read(w: &Widget, name: &str, snap: &Snapshot, now: Instant, atm: f64) -> Read {
    let seen = snap.values.get(name);
    let shown = values::show(name, seen, now);
    let d = defs::describe(name);
    let base_unit = d.unit;
    let num = raw(snap, name, now).filter(|_| matches!(d.kind, Kind::Num(_)));
    let relative = w.opts.relative && absolute_pressure(name);
    let unit = w
        .opts
        .unit
        .clone()
        .filter(|u| units_for(base_unit).contains(&u.as_str()))
        .unwrap_or_else(|| base_unit.to_string());
    let dp = w.opts.decimals.or_else(|| unit_dp(&unit).filter(|_| unit != base_unit)).unwrap_or(match d.kind {
        Kind::Num(dp) => dp,
        _ => 0,
    });
    let x = num.map(|n| {
        let n = if relative { n - atm } else { n };
        convert(n, base_unit, &unit, relative)
    });
    let text = match (x, seen.and_then(|s| s.value.as_ref())) {
        (Some(x), _) => values::number(x, dp),
        (None, Some(_)) => shown.value.clone(),
        (None, None) => NONE.to_string(),
    };
    Read {
        x,
        text,
        unit: if num.is_some() { unit } else { shown.unit.clone() },
        label: shown.label,
        stale: shown.freshness.dim(),
        dp,
    }
}

/// The range `w` draws `name` in, in the widget's unit: (min, max, warn, limit, major).
fn range(w: &Widget, name: &str, snap: &Snapshot, now: Instant, atm: f64, mem: &Memory) -> Scale {
    let d = defs::describe(name);
    let base = d.unit;
    let unit = w
        .opts
        .unit
        .clone()
        .filter(|u| units_for(base).contains(&u.as_str()))
        .unwrap_or_else(|| base.to_string());
    let relative = w.opts.relative && absolute_pressure(name);
    let conv = |x: f64| {
        let x = if relative { x - atm } else { x };
        convert(x, base, &unit, relative)
    };
    let cat = scale(name).map(|s| {
        let (mut min, max) = (conv(s.min), conv(s.max));
        if relative {
            // Relative boost: a little vacuum below zero.
            min = min.max(conv(atm - 1000.0)).min(0.0);
        }
        let k = (conv(s.min + s.major) - conv(s.min)).abs();
        Scale {
            min,
            max,
            warn: s.warn.map(conv),
            limit: s.limit.map(conv),
            major: nice(k),
        }
    });
    let seen = |name: &str| {
        let lo = mem.window_min(name, now, HISTORY);
        let hi = mem.window_max(name, now, HISTORY);
        (lo.map(conv), hi.map(conv))
    };
    let mut s = cat.unwrap_or_else(|| {
        let (lo, hi) = seen(name);
        let (lo, hi) = (lo.unwrap_or(0.0).min(0.0), hi.unwrap_or(100.0).max(1.0));
        Scale {
            min: lo,
            max: hi * 1.1,
            warn: None,
            limit: None,
            major: nice((hi * 1.1 - lo) / 4.0),
        }
    });
    if let Some((a, b)) = w.opts.range.filter(|(a, b)| b > a) {
        s.min = a;
        s.max = b;
        s.major = nice((b - a) / 4.0);
    }
    if w.opts.warn.is_some() {
        s.warn = w.opts.warn;
    }
    if w.opts.limit.is_some() {
        s.limit = w.opts.limit;
    }
    let _ = snap;
    s
}

/// 1, 2, 2.5 or 5 times a power of ten, at least `x`.
fn nice(x: f64) -> f64 {
    if !(x.is_finite() && x > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(x.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if m * p >= x * 0.999 {
            return m * p;
        }
    }
    10.0 * p
}

fn state_of(x: Option<f64>, s: &Scale) -> i32 {
    let Some(x) = x else { return 0 };
    // On a scale that ends at or below zero (timing retard) the limits are lower limits.
    let crossed = |lim: f64| if s.max <= 0.0 { x <= lim } else { x >= lim };
    if s.limit.is_some_and(crossed) {
        2
    } else if s.warn.is_some_and(crossed) {
        1
    } else {
        0
    }
}

fn frac(x: f64, s: &Scale) -> f32 {
    ((x - s.min) / (s.max - s.min)).clamp(0.0, 1.0) as f32
}

/// The gauge arc: 270°, open at the bottom, from 225° (bottom left) clockwise.
const A0: f64 = 135.0;
const SWEEP: f64 = 270.0;
const TICKS: usize = 48;

fn polar(f: f64, r: f64) -> (f64, f64) {
    let a = (A0 + SWEEP * f).to_radians();
    (500.0 + r * a.cos(), 520.0 + r * a.sin())
}

fn seg(path: &mut String, f: f64, r1: f64, r2: f64) {
    let (x1, y1) = polar(f, r1);
    let (x2, y2) = polar(f, r2);
    path.push_str(&format!("M {x1:.0} {y1:.0} L {x2:.0} {y2:.0} "));
}

fn gauge(d: &mut WidgetDraw, x: Option<f64>, s: &Scale, peak: Option<f64>, dp: u8) {
    const R: f64 = 400.0;
    let red_from = s.limit.or(s.warn).map(|r| frac(r, s) as f64);
    let at = x.map(|x| frac(x, s) as f64);
    for i in 0..=TICKS {
        let f = i as f64 / TICKS as f64;
        let red = red_from.is_some_and(|r| f >= r - 1e-9);
        let lit = at.is_some_and(|a| f <= a + 1e-9);
        let path = match (red, lit) {
            (true, true) => &mut d.red_on,
            (true, false) => &mut d.red_off,
            (false, true) => &mut d.ticks_on,
            (false, false) => &mut d.ticks_off,
        };
        seg(path, f, R - 55.0, R);
    }
    // Majors: longer, with numbers.
    let mut v = (s.min / s.major).ceil() * s.major;
    let mut n = 0;
    while v <= s.max + 1e-9 && n < 12 {
        let f = frac(v, s) as f64;
        seg(&mut d.ticks_off, f, R - 90.0, R);
        let (tx, ty) = polar(f, R + 70.0);
        let label_dp = if s.major < 1.0 { dp.max(1) } else { 0 };
        d.labels.push(Tick {
            x: (tx / 1000.0) as f32,
            y: (ty / 1000.0) as f32,
            label: values::number(v, label_dp.min(2)),
        });
        v += s.major;
        n += 1;
    }
    if let Some(a) = at {
        seg(&mut d.needle, a, R - 120.0, R + 8.0);
    }
    if let Some(p) = peak {
        seg(&mut d.peak, frac(p, s) as f64, R - 30.0, R + 30.0);
    }
}

fn bar(d: &mut WidgetDraw, x: Option<f64>, s: &Scale, segments: u8) {
    let n = segments.clamp(4, 20) as i32;
    d.segs = n;
    let at = |v: f64| (frac(v, s) * n as f32).round() as i32;
    d.lit = x.map_or(0, |x| (frac(x, s) * n as f32).ceil() as i32);
    d.warn_from = s.warn.map_or(n, at);
    d.limit_from = s.limit.map_or(n, at);
}

/// A graph line through the samples of the last `window`, in the box: x by time, y by `s`.
fn trace(mem: &Memory, name: &str, window: Duration, now: Instant, s: &Scale, conv: &dyn Fn(f64) -> f64) -> String {
    let mut path = String::new();
    let span = window.as_secs_f64();
    for (t, x) in mem.series(name) {
        let age = now.saturating_duration_since(*t).as_secs_f64();
        if age > span {
            continue;
        }
        let px = 1000.0 * (1.0 - age / span);
        let py = 1000.0 * (1.0 - frac(conv(*x), s) as f64);
        path.push_str(&format!(
            "{} {px:.0} {py:.0} ",
            if path.is_empty() { "M" } else { "L" }
        ));
    }
    path
}

/// A path cut into dashes, for specified values.
fn dashed(path: &str) -> String {
    let pts: Vec<(f64, f64)> = path
        .split(['M', 'L'])
        .filter_map(|p| {
            let mut it = p.split_whitespace().map(|n| n.parse::<f64>().ok());
            Some((it.next()??, it.next()??))
        })
        .collect();
    let mut out = String::new();
    let (on, off): (f64, f64) = (18.0, 14.0);
    let mut phase: f64 = 0.0;

    for w in pts.windows(2) {
        let ((x1, y1), (x2, y2)) = (w[0], w[1]);
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        let mut t = 0.0;
        while t < len {
            let cycle = phase % (on + off);
            let step = if cycle < on { on - cycle } else { on + off - cycle }.min(len - t);
            if cycle < on {
                let a = t / len;
                let b = (t + step) / len;
                out.push_str(&format!(
                    "M {:.0} {:.0} L {:.0} {:.0} ",
                    x1 + (x2 - x1) * a,
                    y1 + (y2 - y1) * a,
                    x1 + (x2 - x1) * b,
                    y1 + (y2 - y1) * b
                ));
            }
            t += step;
            phase += step;
        }
    }
    out
}

/// A specified value draws dashed next to its actual.
fn specified(name: &str) -> bool {
    name.contains("specified")
}

/// Draws widget `w`.
pub fn draw(w: &Widget, snap: &Snapshot, mem: &Memory, now: Instant) -> WidgetDraw {
    let atm = atmosphere(snap, now);
    let mut d = WidgetDraw {
        col: w.col,
        row: w.row,
        w: w.w,
        h: w.h,
        style: w.style.code(),
        focus_at: -1,
        ..WidgetDraw::default()
    };
    let Some(first) = w.values.first() else {
        d.label = "Empty".into();
        return d;
    };
    let r = read(w, first, snap, now, atm);
    d.label = w.opts.label.clone().unwrap_or_else(|| r.label.clone());
    if w.opts.relative && absolute_pressure(first) && w.opts.label.is_none() {
        d.label = d.label.replace(" actual", "").replace("Boost act.", "Boost");
    }
    d.value = r.text.clone();
    d.unit = r.unit.clone();
    d.stale = r.stale;
    let s = range(w, first, snap, now, atm, mem);
    d.state = state_of(r.x, &s);
    d.frac = r.x.map_or(0.0, |x| frac(x, &s));
    let base = defs::describe(first).unit;
    let unit = r.unit.clone();
    let relative = w.opts.relative && absolute_pressure(first);
    let conv = move |x: f64| {
        let x = if relative { x - atm } else { x };
        convert(x, base, &unit, relative)
    };
    match w.style {
        Style::Number => {
            let span = match w.opts.minmax {
                MinMax::Off => None,
                MinMax::SinceStart => Some(HISTORY * 1000),
                MinMax::Last30s => Some(Duration::from_secs(30)),
            };
            if let Some(span) = span {
                let lo = if w.opts.minmax == MinMax::SinceStart {
                    mem.lows.get(first.as_str()).copied()
                } else {
                    mem.window_min(first, now, span)
                };
                let hi = if w.opts.minmax == MinMax::SinceStart {
                    mem.peaks.get(first.as_str()).map(|p| p.0)
                } else {
                    mem.window_max(first, now, span)
                };
                if let (Some(lo), Some(hi)) = (lo, hi) {
                    d.sub = format!(
                        "{} – {}",
                        values::number(conv(lo), r.dp),
                        values::number(conv(hi), r.dp)
                    );
                }
            }
        }
        Style::Gauge => {
            let peak = mem.peak(first, now, w.opts.peak).map(&conv);
            gauge(&mut d, r.x, &s, peak, r.dp);
            if let Some(p) = peak {
                d.sub = format!("PEAK {}", values::number(p, r.dp));
                d.sub_hi = true;
            }
        }
        Style::Bar => bar(&mut d, r.x, &s, w.opts.segments.unwrap_or(10)),
        Style::Graph => {
            let window = Duration::from_secs(u64::from(w.opts.window.unwrap_or(30)));
            // One scale for every line: the first's, widened to what's in the window.
            let mut s = s;
            if !w.opts.fixed {
                let mut lo = f64::INFINITY;
                let mut hi = f64::NEG_INFINITY;
                for n in &w.values {
                    let rel = w.opts.relative && absolute_pressure(n);
                    let b = defs::describe(n).unit;
                    let u = r.unit.clone();
                    for (t, x) in mem.series(n) {
                        if now.saturating_duration_since(*t) <= window {
                            let x = if rel { x - atm } else { *x };
                            let x = convert(x, b, &u, rel);
                            lo = lo.min(x);
                            hi = hi.max(x);
                        }
                    }
                }
                if lo.is_finite() && hi > lo {
                    let pad = (hi - lo) * 0.1;
                    s.min = lo - pad;
                    s.max = hi + pad;
                } else if lo.is_finite() {
                    s.min = lo - 1.0;
                    s.max = hi + 1.0;
                }
            }
            for (i, n) in w.values.iter().enumerate() {
                let rel = w.opts.relative && absolute_pressure(n);
                let b = defs::describe(n).unit;
                let u = r.unit.clone();
                let c = move |x: f64| {
                    let x = if rel { x - atm } else { x };
                    convert(x, b, &u, rel)
                };
                let path = trace(mem, n, window, now, &s, &c);
                let spec = specified(n) && i > 0;
                let rn = read(w, n, snap, now, atm);
                d.lines.push(Line {
                    path: if spec { dashed(&path) } else { path },
                    color: if spec { 1 } else if i == 0 { 0 } else { 2 },
                    label: format!("{} {}", rn.label, rn.text),
                });
            }
            let span = window.as_secs_f64();
            for (n, t) in &mem.marks {
                let age = now.saturating_duration_since(*t).as_secs_f64();
                if age <= span {
                    d.marks.push(Tick {
                        x: (1.0 - age / span) as f32,
                        y: 0.0,
                        label: n.to_string(),
                    });
                }
            }
        }
        Style::Pair => {
            if let Some(second) = w.values.get(1) {
                let r2 = read(w, second, snap, now, atm);
                d.label2 = r2.label.clone();
                d.value2 = r2.text.clone();
                if let (Some(a), Some(b)) = (r.x, r2.x) {
                    d.sub = if w.opts.diff_pct && b != 0.0 {
                        format!("{:+.0} %", (a - b) / b.abs() * 100.0)
                    } else {
                        format!("{:+} {}", values::number(a - b, r.dp), r.unit)
                            .replace("+-", "−")
                    };
                }
            }
        }
        Style::Set => {
            if w.opts.label.is_none() && w.values.len() > 1 {
                let labels: Vec<String> =
                    w.values.iter().map(|n| read(w, n, snap, now, atm).label).collect();
                d.label = shared(&labels);
            }
            for n in &w.values {
                let rn = read(w, n, snap, now, atm);
                let sn = range(w, n, snap, now, atm, mem);
                let on = matches!(
                    snap.values.get(n).and_then(|s| s.value.as_ref()),
                    Some(Value::Bool(true))
                );
                d.items.push(Item {
                    label: short(&rn.label),
                    value: rn.text.clone(),
                    frac: rn.x.map_or(-1.0, |x| frac(x, &sn)),
                    on,
                    state: state_of(rn.x, &sn),
                });
            }
        }
        Style::Selector => {
            let code = raw(snap, first, now);
            let now_at = code.map(defs::selector).unwrap_or_default();
            for p in ["P", "R", "N", "D", "S"] {
                d.items.push(Item {
                    label: p.into(),
                    on: now_at == p,
                    frac: -1.0,
                    ..Item::default()
                });
            }
            // Tiptronic gears and the rest show as the big value.
            d.value = now_at.clone();
            d.unit.clear();
            let shifting = matches!(
                snap.values.get("trans.shifting").and_then(|s| s.value.as_ref()),
                Some(Value::Bool(true))
            );
            d.sub = if shifting { "SHIFTING".into() } else { String::new() };
            d.sub_hi = shifting;
        }
    }
    d
}

/// The words a set's labels start with: "Retard cyl 1".."4" share "Retard cyl" -> "Retard".
fn shared(labels: &[String]) -> String {
    let words: Vec<Vec<&str>> = labels.iter().map(|l| l.split_whitespace().collect()).collect();
    let first = &words[0];
    let mut n = first.len();
    for w in &words[1..] {
        n = n.min(first.iter().zip(w).take_while(|(a, b)| a == b).count());
    }
    let mut out: Vec<&str> = first[..n].to_vec();
    // A trailing short word ("cyl") is the items' own.
    if out.len() > 1 && out.last().is_some_and(|w| w.len() <= 3) {
        out.pop();
    }
    if out.is_empty() {
        labels[0].clone()
    } else {
        out.join(" ")
    }
}

/// A label short enough for a set's item: its last word or code ("FL", "Cyl 1").
fn short(label: &str) -> String {
    let words: Vec<&str> = label.split_whitespace().collect();
    match words.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [.., last] if last.len() <= 3 => (*last).to_string(),
        [.., a, b] => format!("{a} {b}"),
    }
}

/// The values a panel's widgets need sampled: every value, and the atmosphere for relative ones.
pub fn sampled(p: &Panel) -> Vec<String> {
    let mut out = p.values();
    if p
        .widgets
        .iter()
        .any(|w| w.opts.relative && w.values.iter().any(|v| absolute_pressure(v)))
    {
        for a in ["engine.altitude_factor", "engine.atmospheric_mbar"] {
            if !out.iter().any(|v| v == a) {
                out.push(a.into());
            }
        }
    }
    out
}

/// A panel, viewing.
pub fn view(p: &Panel, snap: &Snapshot, mem: &Memory, now: Instant) -> PanelDraw {
    let (cols, rows) = p.grid.size();
    PanelDraw {
        cols,
        rows,
        widgets: p.widgets.iter().map(|w| draw(w, snap, mem, now)).collect(),
        empties: Vec::new(),
        dots: 0,
        at: 0,
        edit: false,
        hint: if p.widgets.is_empty() {
            if p.scratch {
                "Push to add a value".into()
            } else {
                "Push · Edit panel".into()
            }
        } else {
            String::new()
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Acts and screens
// ---------------------------------------------------------------------------------------------

/// What a value picker fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A new one-cell widget at a free cell of the panel being edited.
    Cell(u8, u8),
    /// Value `k` of widget `w`; `k` past the end adds one.
    Value(usize, usize),
    /// A new widget on Scratch.
    Scratch,
}

/// A screen of the Panels tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelScreen {
    /// The panel itself.
    #[default]
    View,
    /// Push on the panel: edit, log, reset peaks, new, the list.
    Menu,
    /// The panel with its widgets and free cells focusable.
    Edit,
    /// One widget's settings beside the panel.
    Widget(usize),
    Pick(Target),
    /// Every panel but Scratch: open, reorder.
    List,
    /// One panel's name, grid and Remove.
    Settings(usize),
}

impl PanelScreen {
    pub fn parent(self) -> Option<PanelScreen> {
        use PanelScreen as P;
        Some(match self {
            P::View => return None,
            P::Menu | P::Edit | P::List => P::View,
            P::Widget(_) => P::Edit,
            P::Pick(Target::Cell(..)) => P::Edit,
            P::Pick(Target::Value(w, _)) => P::Widget(w),
            P::Pick(Target::Scratch) => P::View,
            P::Settings(_) => P::List,
        })
    }

    pub fn depth(self) -> usize {
        std::iter::successors(self.parent(), |s| s.parent()).count()
    }
}

/// What pushing or the D-pad does on the Panels tab.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelAct {
    /// The panel before or after.
    Step(i32),
    Menu,
    Edit,
    /// Scratch: pick a value to add.
    Add,
    /// Scratch becomes a panel of its own; Scratch empties.
    SaveScratch,
    ResetPeaks,
    New,
    List,
    /// The edited panel's grid, a step smaller or bigger.
    Grid(i32),
    /// Opens a widget's settings.
    Widget(usize),
    /// A free cell: pick a value for a new widget there.
    Cell(u8, u8),
    /// Moves a widget to the free spot before or after it.
    Move(usize, i32),
    /// Picks value `k` of widget `w`.
    Value(usize, usize),
    /// Removes value `k` of widget `w`.
    Drop(usize, usize),
    Style(usize, i32),
    Size(usize, i32),
    Decimals(usize, i32),
    Unit(usize, i32),
    Relative(usize),
    MinMax(usize, i32),
    Peak(usize, i32),
    Window(usize, i32),
    Fixed(usize),
    DiffPct(usize),
    Segments(usize, i32),
    /// Range low and high ends, warn and limit, by the range's major step.
    Low(usize, i32),
    High(usize, i32),
    Warn(usize, i32),
    Limit(usize, i32),
    /// Asks, then removes the widget.
    Remove(usize),
    /// The panel list: open one's settings, reorder.
    Open(usize),
    Reorder(usize, i32),
    Name(usize, i32),
    PanelGrid(usize, i32),
    /// Asks, then removes the panel.
    RemovePanel(usize),
    /// Shows panel `i` and goes back to it.
    Show(usize),
}

/// The scale of value `name` in widget `w`, for the editor's steps.
pub fn scale_of(w: &Widget, name: &str, snap: &Snapshot, mem: &Memory, now: Instant) -> Scale {
    let atm = atmosphere(snap, now);
    range(w, name, snap, now, atm, mem)
}

/// The sizes widget `i` can take where it sits, smallest first.
pub fn sizes(p: &Panel, i: usize) -> Vec<(u8, u8)> {
    let Some(w) = p.widgets.get(i) else {
        return Vec::new();
    };
    let (cols, rows) = p.grid.size();
    let mut out = Vec::new();
    for h in 1..=rows {
        for ww in 1..=cols {
            if p.fits(Some(i), w.col, w.row, ww, h) {
                out.push((ww, h));
            }
        }
    }
    out.sort_by_key(|(a, b)| (u16::from(*a) * u16::from(*b), *b));
    out
}

/// Moves widget `i` to the next spot in reading order (`d` > 0) or the one before where it fits.
pub fn shift(p: &mut Panel, i: usize, d: i32) -> bool {
    let Some(w) = p.widgets.get(i).cloned() else {
        return false;
    };
    let (cols, rows) = p.grid.size();
    let n = i32::from(cols) * i32::from(rows);
    let at = i32::from(w.row) * i32::from(cols) + i32::from(w.col);
    let mut k = at + d.signum();
    while (0..n).contains(&k) {
        let (c, r) = ((k % i32::from(cols)) as u8, (k / i32::from(cols)) as u8);
        if p.fits(Some(i), c, r, w.w, w.h) {
            p.widgets[i].col = c;
            p.widgets[i].row = r;
            return true;
        }
        k += d.signum();
    }
    false
}

#[cfg(test)]
mod tests {
    use carpi_proto::Reading;

    use super::*;

    fn snap(values: &[(&str, f64)]) -> Snapshot {
        let mut s = Snapshot {
            connected: true,
            ..Snapshot::default()
        };
        s.values.take(
            values
                .iter()
                .map(|(n, v)| Reading {
                    name: n.to_string(),
                    value: Some(Value::Num(*v)),
                    age_ms: 0,
                })
                .collect(),
            Instant::now(),
        );
        s
    }

    #[test]
    fn relative_boost_subtracts_icans_atmosphere() {
        let s = snap(&[
            ("engine.boost.actual", 1850.0),
            ("engine.altitude_factor", 0.96),
            ("engine.atmospheric_mbar", 900.0),
        ]);
        let now = Instant::now();
        let mut w = Widget::new(0, 0, Style::Number, &["engine.boost.actual"]);
        w.opts.relative = true;
        w.opts.unit = Some("bar".into());
        let d = draw(&w, &s, &Memory::default(), now);
        // 1850 - 0.96 × 1013.25 = 877 mbar.
        assert_eq!((d.value.as_str(), d.unit.as_str()), ("0.88", "bar"));
        w.opts.relative = false;
        w.opts.unit = Some("psi".into());
        let d = draw(&w, &s, &Memory::default(), now);
        assert_eq!(d.value, "26.8");
        // No ICAN factor: the measuring block's.
        let s = snap(&[("engine.boost.actual", 800.0), ("engine.atmospheric_mbar", 1000.0)]);
        w.opts.relative = true;
        w.opts.unit = None;
        let d = draw(&w, &s, &Memory::default(), now);
        assert_eq!(d.value, "-200");
    }

    #[test]
    fn gauges_light_ticks_to_the_value_and_redden_past_the_limit() {
        let s = snap(&[("engine.rpm", 6800.0)]);
        let mut w = Widget::new(0, 0, Style::Gauge, &["engine.rpm"]);
        w.opts.peak = Peak::Off;
        let d = draw(&w, &s, &Memory::default(), Instant::now());
        assert_eq!(d.state, 2);
        assert!(!d.red_on.is_empty() && !d.ticks_on.is_empty() && !d.needle.is_empty());
        assert_eq!(d.labels.first().map(|t| t.label.as_str()), Some("0"));
        assert_eq!(d.labels.last().map(|t| t.label.as_str()), Some("7000"));
        assert!(d.sub.is_empty());
    }

    #[test]
    fn bars_count_segments_and_limits() {
        let s = snap(&[("engine.coolant_c", 108.0)]);
        let w = Widget::new(0, 0, Style::Bar, &["engine.coolant_c"]);
        let d = draw(&w, &s, &Memory::default(), Instant::now());
        // 40..130 in 10: 108 lights 8, amber from 105 (7), red from 115 (8).
        assert_eq!((d.segs, d.lit, d.warn_from, d.limit_from), (10, 8, 7, 8));
        assert_eq!(d.state, 1);
    }

    #[test]
    fn graphs_trace_history_and_dash_specified() {
        let now = Instant::now();
        let mut mem = Memory::default();
        let names = vec![
            "engine.boost.actual".to_string(),
            "engine.boost.specified".to_string(),
        ];
        for i in 0..20 {
            let s = snap(&[
                ("engine.boost.actual", 1000.0 + 50.0 * i as f64),
                ("engine.boost.specified", 1500.0),
            ]);
            mem.sample(&names, &s, now - Duration::from_millis(100 * (20 - i)));
        }
        mem.marks.push((3, now - Duration::from_secs(1)));
        let s = snap(&[("engine.boost.actual", 2000.0), ("engine.boost.specified", 1500.0)]);
        let w = Widget::new(0, 0, Style::Graph, &["engine.boost.actual", "engine.boost.specified"]);
        let d = draw(&w, &s, &mem, now);
        assert_eq!(d.lines.len(), 2);
        assert!(d.lines[0].path.starts_with("M "));
        assert_eq!(d.lines[1].color, 1);
        assert!(d.lines[1].path.matches('M').count() > 3, "dashes");
        assert_eq!(d.marks[0].label, "3");
    }

    #[test]
    fn restyling_keeps_what_applies() {
        let mut w = Widget::new(0, 0, Style::Gauge, &["engine.rpm"]);
        w.opts.range = Some((0.0, 8000.0));
        w.opts.label = Some("Revs".into());
        w.restyle(Style::Graph);
        assert_eq!(w.opts.range, Some((0.0, 8000.0)));
        w.restyle(Style::Number);
        assert_eq!(w.opts.range, None);
        assert_eq!(w.opts.label.as_deref(), Some("Revs"));
    }

    #[test]
    fn scratch_lays_itself_out_and_grids_clamp() {
        let mut p = Panel::scratch();
        for n in ["a", "b", "c"] {
            p.widgets.push(Widget::new(0, 0, Style::Number, &[n]));
        }
        p.relayout();
        assert_eq!(p.grid, Grid::G2x2);
        assert_eq!((p.widgets[2].col, p.widgets[2].row), (0, 1));
        assert_eq!(p.free(), vec![(1, 1)]);
        let mut p = Panel::new("x", Grid::G3x2, vec![Widget::new(0, 0, Style::Gauge, &["a"]).size(2, 2), Widget::new(2, 1, Style::Number, &["b"])]);
        p.grid = Grid::G2x1;
        p.clamp();
        assert_eq!(p.widgets.len(), 1);
        assert_eq!((p.widgets[0].w, p.widgets[0].h), (2, 1));
    }

    #[test]
    fn selectors_light_the_position() {
        let s = snap(&[("trans.selector", 5.0)]);
        let w = Widget::new(0, 0, Style::Selector, &["trans.selector"]);
        let d = draw(&w, &s, &Memory::default(), Instant::now());
        let lit: Vec<&str> = d.items.iter().filter(|i| i.on).map(|i| i.label.as_str()).collect();
        assert_eq!(lit, ["D"]);
        assert_eq!(d.value, "D");
    }
}
