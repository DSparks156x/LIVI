// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! A page as plain data, for livi-ui to copy into `CarpiState`.
//!
//! Everything focusable has an index in one list, in screen order: header tabs, then grid tiles,
//! then rows. The knob steps through it, push presses what's focused, and the D-pad runs the
//! focused thing's `side` acts (or the view's own, when it has none).

use crate::slots::SlotAct;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Rows, with the grid on top when the view has tiles.
    List,
    /// Only the grid, filling the page.
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A caption over the rows that follow it.
    Section,
    /// A label and a reading.
    Info,
    /// Opens a screen.
    Route,
    /// Does something; `button` is the action's word.
    Button,
    Toggle,
    /// A choice the D-pad cycles in place; push steps it forward.
    Choice,
    /// A number the D-pad steps in place.
    Step,
}

/// What pressing something does. The page that built the view knows what each one means.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    None,
    Edit,
    Module(usize),
    Car(CarScreen),
    Capture,
    /// Sends a settings change to carpid; with `true`, goes back a screen too.
    Patch(Box<carpi_proto::SettingsPatch>, bool),
    PinDigit(u8),
    PinDelete,
    PinSave,
    /// Starts a diag job.
    Diag(carpi_proto::DiagAction),
    /// Asks to confirm clearing a module's fault codes, or doesn't.
    ClearAsk(u8),
    ClearCancel,
    /// A brightness nudge or "reset learned".
    Light(carpi_proto::LightAction),
    /// Shows or hides the manual curve's points.
    Advanced,
    /// The slot editor and the picker.
    Slots(SlotAct),
    /// Data logging: the run and the profiles.
    Log(crate::logging::LogAct),
    /// Changes to the cluster display's readings pages.
    Dis(DisAct),
    /// Sets the time zone and goes back to the Power & clock screen.
    Zone(String),
    /// Starts an update (check, download, install) and goes back.
    Update,
    /// Back a screen.
    Back,
}

/// A readings page change: page `p`, cell `c`.
#[derive(Debug, Clone, PartialEq)]
pub enum DisAct {
    AddPage,
    /// Asks, then removes the page.
    RemovePage(usize),
    /// Moves page `p` to position `to`.
    MovePage(usize, usize),
    Name(usize, i32),
    MoveCell(usize, usize, i32),
    Decimals(usize, usize, u8),
    Unit(usize, usize, Option<String>),
    /// Sets the icon and goes back.
    Icon(usize, usize, Option<String>),
    RemoveCell(usize, usize),
}

/// A screen of the Car Integration tab (docs/PAGES.md 5a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarScreen {
    Root,
    Status,
    AllValues,
    Buses,
    Cluster,
    /// The readings pages, one page, one value on it, its picker, a new value's picker, its
    /// icon.
    ReadingPages,
    ReadingPage(usize),
    Cell(usize, usize),
    CellValue(usize, usize),
    CellAdd(usize),
    CellIcon(usize, usize),
    Nav,
    Calls,
    Media,
    Lighting,
    /// The plot and the manual curve's points, and one point.
    Curve,
    Point(usize),
    Night,
    Controls,
    Wheel,
    Faceplate,
    /// One key's bindings ([`crate::keys::KEYS`] index), and the action picker for one row.
    Key(usize),
    Binding(usize, usize),
    Logs,
    /// Data logging: the run and the profiles, one profile, its channels.
    Logging,
    LogProfile,
    LogChannels,
    Power,
    /// Time zone regions, and the zones in one.
    Zones,
    Zone(usize),
    Avsd,
    Picture,
    Portal,
    /// Entering a new portal PIN.
    Pin,
    Updates,
    UpdateConfirm,
    Advanced,
}

impl CarScreen {
    /// The screen back goes to; `None` on the first one.
    pub fn parent(self) -> Option<CarScreen> {
        use CarScreen as S;
        Some(match self {
            S::Root => return None,
            S::AllValues | S::Buses => S::Status,
            S::ReadingPages | S::Nav | S::Calls | S::Media => S::Cluster,
            S::ReadingPage(_) => S::ReadingPages,
            S::Cell(p, _) | S::CellAdd(p) => S::ReadingPage(p),
            S::CellValue(p, c) | S::CellIcon(p, c) => S::Cell(p, c),
            S::Curve | S::Night => S::Lighting,
            S::Point(_) => S::Curve,
            S::Wheel | S::Faceplate => S::Controls,
            S::Key(k) => match crate::keys::KEYS.get(k) {
                Some(key) if key.wheel => S::Wheel,
                _ => S::Faceplate,
            },
            S::Binding(k, _) => S::Key(k),
            S::Logging => S::Logs,
            S::LogProfile => S::Logging,
            S::LogChannels => S::LogProfile,
            S::Zones => S::Power,
            S::Zone(_) => S::Zones,
            S::Picture => S::Avsd,
            S::Pin => S::Portal,
            S::UpdateConfirm => S::Updates,
            _ => S::Root,
        })
    }

    /// Screens below the first one.
    pub fn depth(self) -> usize {
        std::iter::successors(self.parent(), |s| s.parent()).count()
    }

    /// The light screens: carpid's focus is `page:light` while one is up.
    pub fn light(self) -> bool {
        matches!(
            self,
            CarScreen::Lighting | CarScreen::Curve | CarScreen::Point(_) | CarScreen::Night
        )
    }
}

/// The two acts of the D-pad: left, right.
pub type Side = Option<(Act, Act)>;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub label: String,
    pub value: String,
    /// Word on a button row.
    pub button: String,
    /// A small tag after the label, like `diag`.
    pub tag: String,
    pub dimmed: bool,
    pub alert: bool,
    /// A toggle that's on, or the value already in a slot.
    pub checked: bool,
    pub mono: bool,
    /// Index in the focus list, -1 for a section.
    pub focus_at: i32,
    /// First and last row of its section, for the rounded corners.
    pub first: bool,
    pub last: bool,
    pub act: Act,
    pub side: Side,
    /// A LIVI icon name, for the tab's own rows.
    pub icon: Option<&'static str>,
}

impl Row {
    pub fn new(kind: RowKind, label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            value: value.into(),
            button: String::new(),
            tag: String::new(),
            dimmed: false,
            alert: false,
            checked: false,
            mono: false,
            focus_at: -1,
            first: false,
            last: false,
            act: Act::None,
            side: None,
            icon: None,
        }
    }

    pub fn act(mut self, act: Act) -> Self {
        self.act = act;
        self
    }

    pub fn side(mut self, left: Act, right: Act) -> Self {
        self.side = Some((left, right));
        self
    }

    pub fn dimmed(mut self, on: bool) -> Self {
        self.dimmed = on;
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    pub fn button(mut self, word: impl Into<String>) -> Self {
        self.button = word.into();
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = tag.into();
        self
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn alert(mut self, on: bool) -> Self {
        self.alert = on;
        self
    }
}

/// A tab or a button in the header.
#[derive(Debug, Clone, PartialEq)]
pub struct Pill {
    pub label: String,
    /// The tab shown; buttons are never active.
    pub active: bool,
    pub focus_at: i32,
    pub act: Act,
    pub side: Side,
}

/// One tile of the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub label: String,
    pub value: String,
    pub unit: String,
    /// One line in place of the value when there isn't one: "not seen", "no value".
    pub why: String,
    /// Stale or invalid: greyed.
    pub dim: bool,
    /// The slot has nothing in it.
    pub empty: bool,
    /// -1 when tiles don't take focus (the plain Data page).
    pub focus_at: i32,
    pub act: Act,
    pub side: Side,
}

impl Cell {
    pub fn blank() -> Self {
        Self {
            label: String::new(),
            value: String::new(),
            unit: String::new(),
            why: String::new(),
            dim: false,
            empty: true,
            focus_at: -1,
            act: Act::None,
            side: None,
        }
    }
}

/// Where a focusable thing is, to keep it in view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    Header,
    /// A tile on this line of the grid.
    Tile(i32),
    /// A row with this many plain rows and sections above it.
    Row(i32, i32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub layout: Layout,
    pub title: String,
    /// Right of the header: what the D-pad does here ("◀ Recent   Driving ▶").
    pub hint: String,
    /// One line under the header: why something is missing.
    pub note: String,
    /// A back arrow, for touch. RETURN does the same.
    pub back: bool,
    pub pills: Vec<Pill>,
    pub cells: Vec<Cell>,
    /// Grid columns.
    pub cols: usize,
    /// Tiles have outlines: the slot editor.
    pub outlined: bool,
    pub rows: Vec<Row>,
    /// Drawn left of the rows, on the light page.
    pub plot: Option<crate::light::Plot>,
    /// Per focus index, where it is.
    pub spots: Vec<Spot>,
    /// The D-pad's acts where the focused thing has none.
    pub side: Side,
    /// Focus index to start at when the view opens.
    pub start: i32,
    /// Drawn by our own Slint page, not as LIVI settings rows: the picker.
    pub custom: bool,
    /// A pick from a list: LIVI's select screen, the checked row is the current one.
    pub select: bool,
}

/// What a focus index points at.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Back,
    Pill(usize),
    Cell(usize),
    Row(usize),
}

impl View {
    pub fn new(layout: Layout, title: impl Into<String>, back: bool) -> Self {
        Self {
            layout,
            title: title.into(),
            hint: String::new(),
            note: String::new(),
            back,
            pills: Vec::new(),
            cells: Vec::new(),
            cols: 1,
            outlined: false,
            rows: Vec::new(),
            plot: None,
            spots: Vec::new(),
            side: None,
            start: -1,
            custom: false,
            select: false,
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    fn take(&mut self, spot: Spot) -> i32 {
        self.spots.push(spot);
        self.spots.len() as i32 - 1
    }

    /// A header tab or button. Pills come first in the focus order.
    pub fn pill(&mut self, label: impl Into<String>, active: bool, act: Act) -> &mut Pill {
        debug_assert!(self.cells.is_empty() && self.rows.is_empty());
        let focus_at = self.take(Spot::Header);
        self.pills.push(Pill {
            label: label.into(),
            active,
            focus_at,
            act,
            side: None,
        });
        self.pills.last_mut().expect("just pushed")
    }

    /// A grid tile; `focus` makes it take focus. Tiles go after pills and before rows.
    pub fn cell(&mut self, mut cell: Cell, focus: bool) -> i32 {
        debug_assert!(self.rows.is_empty(), "tiles go before rows");
        cell.focus_at = if focus {
            let line = (self.cells.len() / self.cols.max(1)) as i32;
            self.take(Spot::Tile(line))
        } else {
            -1
        };
        self.cells.push(cell);
        cell_at(self)
    }

    pub fn section(&mut self, title: impl Into<String>) {
        self.rows.push(Row::new(RowKind::Section, title, ""));
    }

    /// Adds a row; returns its focus index, -1 for a section.
    pub fn row(&mut self, mut row: Row) -> i32 {
        if row.kind != RowKind::Section {
            let plain = self
                .rows
                .iter()
                .filter(|r| r.kind != RowKind::Section)
                .count();
            let sections = self.rows.len() - plain;
            row.focus_at = self.take(Spot::Row(plain as i32, sections as i32));
        }
        let at = row.focus_at;
        self.rows.push(row);
        at
    }

    /// Sets the corners and the start once everything is in.
    pub fn finish(mut self) -> Self {
        let n = self.rows.len();
        for i in 0..n {
            if self.rows[i].kind == RowKind::Section {
                continue;
            }
            self.rows[i].first = i == 0 || self.rows[i - 1].kind == RowKind::Section;
            self.rows[i].last = i + 1 == n || self.rows[i + 1].kind == RowKind::Section;
        }
        if self.start < 0 || self.start >= self.targets() as i32 {
            self.start = self.first_body();
        }
        self
    }

    /// How many things take focus.
    pub fn targets(&self) -> usize {
        self.spots.len()
    }

    /// The first tile or row; the first pill when there are none.
    pub fn first_body(&self) -> i32 {
        self.spots
            .iter()
            .position(|s| *s != Spot::Header)
            .unwrap_or(0) as i32
    }

    pub fn hit(&self, at: i32) -> Option<Hit> {
        if at < 0 {
            return None;
        }
        if let Some(i) = self.pills.iter().position(|p| p.focus_at == at) {
            return Some(Hit::Pill(i));
        }
        if let Some(i) = self.cells.iter().position(|c| c.focus_at == at) {
            return Some(Hit::Cell(i));
        }
        self.rows
            .iter()
            .position(|r| r.focus_at == at)
            .map(Hit::Row)
    }

    /// What pushing `hit` does.
    pub fn act(&self, hit: &Hit) -> Option<&Act> {
        let act = match hit {
            Hit::Back => return None,
            Hit::Pill(i) => &self.pills.get(*i)?.act,
            Hit::Cell(i) => &self.cells.get(*i)?.act,
            Hit::Row(i) => &self.rows.get(*i)?.act,
        };
        (*act != Act::None).then_some(act)
    }

    /// What the D-pad does on `hit`: `delta` < 0 is left.
    pub fn side_act(&self, hit: Option<&Hit>, delta: i32) -> Option<&Act> {
        let own = match hit {
            Some(Hit::Pill(i)) => self.pills.get(*i).and_then(|p| p.side.as_ref()),
            Some(Hit::Cell(i)) => self.cells.get(*i).and_then(|c| c.side.as_ref()),
            Some(Hit::Row(i)) => self.rows.get(*i).and_then(|r| r.side.as_ref()),
            _ => None,
        };
        let (l, r) = own.or(self.side.as_ref())?;
        let act = if delta < 0 { l } else { r };
        (*act != Act::None).then_some(act)
    }

    /// The focused thing steps with the D-pad and does nothing on push: a held edit, when the
    /// D-pad and the knob send the same keys.
    pub fn steps_only(&self, hit: &Hit) -> bool {
        matches!(hit, Hit::Row(i) if self.rows.get(*i).is_some_and(|r| r.kind == RowKind::Step))
    }
}

fn cell_at(v: &View) -> i32 {
    v.cells.last().map_or(-1, |c| c.focus_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_runs_pills_tiles_rows() {
        let mut v = View::new(Layout::List, "t", true);
        v.pill("Tab", true, Act::Edit);
        v.cols = 2;
        v.cell(Cell::blank(), true);
        v.cell(Cell::blank(), true);
        v.cell(Cell::blank(), true);
        v.section("A");
        v.row(Row::new(RowKind::Info, "a1", ""));
        v.row(Row::new(RowKind::Info, "a2", ""));
        v.section("B");
        v.row(Row::new(RowKind::Toggle, "b1", "").act(Act::Advanced));
        let v = v.finish();
        assert_eq!(v.targets(), 7);
        assert_eq!(v.start, 1);
        assert_eq!(v.hit(0), Some(Hit::Pill(0)));
        assert_eq!(v.hit(3), Some(Hit::Cell(2)));
        assert_eq!(v.hit(4), Some(Hit::Row(1)));
        assert_eq!(v.hit(6), Some(Hit::Row(4)));
        assert_eq!(v.hit(7), None);
        assert_eq!(v.spots[3], Spot::Tile(1));
        assert_eq!(v.spots[5], Spot::Row(1, 1));
        assert_eq!(v.spots[6], Spot::Row(2, 2));
        assert!(v.rows[1].first && !v.rows[1].last);
        assert!(v.rows[4].first && v.rows[4].last);
        assert_eq!(v.act(&Hit::Row(4)), Some(&Act::Advanced));
        assert_eq!(v.act(&Hit::Row(1)), None);
    }

    #[test]
    fn the_dpad_takes_the_focused_acts_then_the_views() {
        let mut v = View::new(Layout::List, "t", false);
        v.side = Some((Act::ClearCancel, Act::Advanced));
        v.row(Row::new(RowKind::Step, "s", "").side(Act::PinDelete, Act::PinSave));
        v.row(Row::new(RowKind::Info, "i", ""));
        let v = v.finish();
        assert_eq!(v.side_act(Some(&Hit::Row(0)), -1), Some(&Act::PinDelete));
        assert_eq!(v.side_act(Some(&Hit::Row(1)), 1), Some(&Act::Advanced));
        assert!(v.steps_only(&Hit::Row(0)));
        assert!(!v.steps_only(&Hit::Row(1)));
    }
}
