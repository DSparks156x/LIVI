// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! A page as plain data, for livi-ui to copy into `CarpiState`.
//!
//! Focus works like LIVI's settings: everything focusable has an index in one list, in screen
//! order (back, then the header pills, then the rows), and the knob steps through it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    List,
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A caption over the rows that follow it.
    Section,
    Info,
    /// Opens a screen.
    Route,
    Button,
    Toggle,
}

/// What pressing something does. The page that built the view knows what each one means.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    None,
    Group(&'static str),
    Edit,
    Reset,
    Toggle(String),
    Module(usize),
    Car(CarScreen),
    Capture,
    /// Sends a settings change to carpid; with `true`, goes back a screen too.
    Patch(Box<carpi_proto::SettingsPatch>, bool),
    PinDigit(u8),
    PinDelete,
    PinSave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarScreen {
    Root,
    Buses,
    Light,
    Rnse,
    Bridge,
    Settings,
    /// The cluster display's readings, and one slot of them.
    Readings,
    Reading(usize),
    /// The key binding table, and one row of it.
    Bindings,
    Binding(usize),
    /// Entering a new portal PIN.
    Pin,
}

impl CarScreen {
    /// The screen back goes to; `None` on the first one.
    pub fn parent(self) -> Option<CarScreen> {
        use CarScreen as S;
        match self {
            S::Root => None,
            S::Readings | S::Bindings | S::Pin => Some(S::Settings),
            S::Reading(_) => Some(S::Readings),
            S::Binding(_) => Some(S::Bindings),
            _ => Some(S::Root),
        }
    }

    /// Screens below the first one.
    pub fn depth(self) -> usize {
        std::iter::successors(self.parent(), |s| s.parent()).count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub label: String,
    pub value: String,
    /// Text of a button row.
    pub button: String,
    pub enabled: bool,
    pub dimmed: bool,
    pub alert: bool,
    pub checked: bool,
    pub mono: bool,
    /// Index in the focus list, -1 for a section.
    pub focus_at: i32,
    /// First and last row of its section, for the rounded corners.
    pub first: bool,
    pub last: bool,
    pub act: Act,
}

impl Row {
    pub fn new(kind: RowKind, label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            value: value.into(),
            button: String::new(),
            enabled: true,
            dimmed: false,
            alert: false,
            checked: false,
            mono: false,
            focus_at: -1,
            first: false,
            last: false,
            act: Act::None,
        }
    }

    pub fn act(mut self, act: Act) -> Self {
        self.act = act;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn dimmed(mut self, on: bool) -> Self {
        self.dimmed = on;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pill {
    pub label: String,
    pub active: bool,
    pub enabled: bool,
    pub focus_at: i32,
    pub act: Act,
}

/// One value of the data grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub label: String,
    pub name: String,
    pub value: String,
    pub unit: String,
    pub age: String,
    /// Stale, invalid or missing.
    pub dim: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub layout: Layout,
    pub title: String,
    /// One line under the header: why something is missing.
    pub note: String,
    /// Focus index of the back arrow, -1 without one.
    pub back_at: i32,
    pub pills: Vec<Pill>,
    pub rows: Vec<Row>,
    pub cells: Vec<Cell>,
    /// For each focusable row in order: plain rows and sections above it, to scroll it into view.
    pub scroll: Vec<(i32, i32)>,
    targets: i32,
}

/// What a focus index points at.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Back,
    Pill(usize),
    Row(usize),
}

impl View {
    pub fn new(layout: Layout, title: impl Into<String>, back: bool) -> Self {
        Self {
            layout,
            title: title.into(),
            note: String::new(),
            back_at: if back { 0 } else { -1 },
            pills: Vec::new(),
            rows: Vec::new(),
            cells: Vec::new(),
            scroll: Vec::new(),
            targets: i32::from(back),
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    /// Pills come before rows in the focus order; add them first.
    pub fn pill(&mut self, label: impl Into<String>, active: bool, enabled: bool, act: Act) {
        debug_assert!(self.rows.is_empty(), "pills go before rows");
        self.pills.push(Pill {
            label: label.into(),
            active,
            enabled,
            focus_at: self.targets,
            act,
        });
        self.targets += 1;
    }

    pub fn section(&mut self, title: impl Into<String>) {
        self.rows.push(Row::new(RowKind::Section, title, ""));
    }

    pub fn row(&mut self, mut row: Row) {
        let plain = self
            .rows
            .iter()
            .filter(|r| r.kind != RowKind::Section)
            .count();
        let sections = self.rows.len() - plain;
        if row.kind != RowKind::Section {
            row.focus_at = self.targets;
            self.targets += 1;
            self.scroll.push((plain as i32, sections as i32));
        }
        self.rows.push(row);
    }

    /// Sets the corners once all rows are in.
    pub fn finish(mut self) -> Self {
        let n = self.rows.len();
        for i in 0..n {
            if self.rows[i].kind == RowKind::Section {
                continue;
            }
            self.rows[i].first = i == 0 || self.rows[i - 1].kind == RowKind::Section;
            self.rows[i].last = i + 1 == n || self.rows[i + 1].kind == RowKind::Section;
        }
        self
    }

    /// How many things take focus.
    pub fn targets(&self) -> usize {
        self.targets as usize
    }

    /// Focus index of the first row, or of the first thing after back.
    pub fn first_target(&self) -> i32 {
        if self.back_at == 0 && self.targets > 1 {
            1
        } else {
            0
        }
    }

    /// Focus index of the first row; without rows, the first thing after back.
    pub fn first_row(&self) -> i32 {
        self.rows
            .iter()
            .find(|r| r.focus_at >= 0)
            .map_or(self.first_target(), |r| r.focus_at)
    }

    pub fn hit(&self, at: i32) -> Option<Hit> {
        if at < 0 {
            return None;
        }
        if at == self.back_at {
            return Some(Hit::Back);
        }
        if let Some(i) = self.pills.iter().position(|p| p.focus_at == at) {
            return Some(Hit::Pill(i));
        }
        self.rows
            .iter()
            .position(|r| r.focus_at == at)
            .map(Hit::Row)
    }

    /// What pressing `hit` does, when it's enabled.
    pub fn act(&self, hit: &Hit) -> Option<&Act> {
        match hit {
            Hit::Back => None,
            Hit::Pill(i) => self.pills.get(*i).filter(|p| p.enabled).map(|p| &p.act),
            Hit::Row(i) => self.rows.get(*i).filter(|r| r.enabled).map(|r| &r.act),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_runs_back_pills_rows() {
        let mut v = View::new(Layout::List, "t", true);
        v.pill("Reset", false, true, Act::Reset);
        v.section("A");
        v.row(Row::new(RowKind::Info, "a1", ""));
        v.row(Row::new(RowKind::Info, "a2", ""));
        v.section("B");
        v.row(Row::new(RowKind::Toggle, "b1", "").act(Act::Toggle("x".into())));
        let v = v.finish();
        assert_eq!(v.targets(), 5);
        assert_eq!(v.first_target(), 1);
        assert_eq!(v.hit(0), Some(Hit::Back));
        assert_eq!(v.hit(1), Some(Hit::Pill(0)));
        assert_eq!(v.hit(2), Some(Hit::Row(1)));
        assert_eq!(v.hit(4), Some(Hit::Row(4)));
        assert_eq!(v.hit(5), None);
        assert_eq!(v.scroll, [(0, 1), (1, 1), (2, 2)]);
        assert!(v.rows[1].first && !v.rows[1].last);
        assert!(!v.rows[2].first && v.rows[2].last);
        assert!(v.rows[4].first && v.rows[4].last);
        assert_eq!(v.act(&Hit::Row(4)), Some(&Act::Toggle("x".into())));
    }

    #[test]
    fn disabled_things_do_nothing() {
        let mut v = View::new(Layout::List, "t", false);
        v.pill("Scan", false, false, Act::Edit);
        v.row(
            Row::new(RowKind::Button, "Read", "")
                .enabled(false)
                .act(Act::Capture),
        );
        assert_eq!(v.first_target(), 0);
        assert_eq!(v.act(&Hit::Pill(0)), None);
        assert_eq!(v.act(&Hit::Row(0)), None);
    }
}
