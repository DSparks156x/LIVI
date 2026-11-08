// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpi fork: carpid's pages in the tab bar, and the Car Integration tab in Settings.
//!
//! Data takes the dashboard's place and Diagnostics goes before Settings. The tab is the last
//! route of Settings' root; its screens come from livi-carpi-ui as views, drawn as LIVI's own
//! rows ([`native`]) or, for the plot and the picker, by our own page over Settings
//! ([`overlay`]).

use livi_carpi_ui::PageId;
use livi_carpi_ui::view::{RowKind as Ours, View};

use crate::settings::node::{Kind, Label, Node};
use crate::settings::view::{Pick, Row, RowKind, Screen};
use crate::shell::Tab;

pub fn tabs(mut tabs: Vec<Tab>) -> Vec<Tab> {
    let data = tabs.iter().position(|t| *t == Tab::Telemetry);
    tabs.retain(|t| *t != Tab::Telemetry);
    tabs.insert(data.unwrap_or(1).min(tabs.len()), Tab::Carpi(PageId::Data));
    let at = tabs
        .iter()
        .position(|t| *t == Tab::Settings)
        .unwrap_or(tabs.len());
    tabs.insert(at, Tab::Carpi(PageId::Diagnostics));
    tabs
}

/// The tab's route key: `/settings/carpi`.
pub const ROUTE: &str = "carpi";

/// Settings' root with the Car Integration tab last, and the tab's index in it.
pub fn with_tab(mut root: Node) -> (Node, usize) {
    let tab = Node::route(ROUTE, Label::plain("Car Integration"), vec![]).icon("carName");
    let mut at = 0;
    if let Kind::Route { children, .. } = &mut root.kind {
        at = children.len();
        children.push(tab);
    }
    (root, at)
}

/// Drawn by our own page over Settings: the plot, tiles, header pills, or the picker.
pub fn overlay(v: &View) -> bool {
    v.custom || v.plot.is_some() || !v.cells.is_empty() || !v.pills.is_empty()
}

/// A view as LIVI settings rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Native {
    pub screen: Screen,
    /// Per LIVI row (or pick), our row; `None` for the note.
    pub rows: Vec<Option<usize>>,
    /// Per our row, its place among the rows that take focus.
    pub slots: Vec<Option<usize>>,
}

impl Native {
    /// Our row under LIVI's row `i`.
    pub fn row(&self, i: usize) -> Option<usize> {
        self.rows.get(i).copied().flatten()
    }

    /// Our row at focus place `slot` (LIVI's focus index less its base).
    pub fn at_slot(&self, slot: usize) -> Option<usize> {
        self.slots.iter().position(|s| *s == Some(slot))
    }

    /// The focus place of our focus index `at` (a row's `focus_at`).
    pub fn slot_of(&self, v: &View, at: i32) -> Option<usize> {
        match v.hit(at)? {
            livi_carpi_ui::view::Hit::Row(r) => self.slots.get(r).copied().flatten(),
            _ => None,
        }
    }
}

fn kind(r: &livi_carpi_ui::view::Row) -> Option<RowKind> {
    use livi_carpi_ui::view::Act;
    Some(match r.kind {
        Ours::Section => return None,
        Ours::Info if r.act == Act::None => RowKind::Info,
        Ours::Info => RowKind::Pick,
        Ours::Route => RowKind::Route,
        Ours::Button => RowKind::Button,
        Ours::Toggle => RowKind::Switch,
        Ours::Choice | Ours::Step => RowKind::Value,
    })
}

pub fn native(v: &View) -> Native {
    let mut rows = Vec::new();
    let mut slots = vec![None; v.rows.len()];
    if v.select {
        let mut options = Vec::new();
        for (i, r) in v.rows.iter().enumerate() {
            if r.kind == Ours::Section {
                continue;
            }
            slots[i] = Some(options.len());
            rows.push(Some(i));
            options.push(Pick {
                label: r.label.clone(),
                selected: r.checked,
                offline: false,
            });
        }
        return Native {
            screen: Screen::Select {
                title: v.title.clone(),
                options,
                read_only: false,
            },
            rows,
            slots,
        };
    }
    let mut out = Vec::new();
    if !v.note.is_empty() {
        rows.push(None);
        out.push(Row {
            kind: RowKind::Info,
            label: v.note.clone(),
            enabled: true,
            ..Default::default()
        });
    }
    let mut focus = 0;
    for (i, r) in v.rows.iter().enumerate() {
        let Some(k) = kind(r) else { continue };
        if !matches!(k, RowKind::Info) {
            slots[i] = Some(focus);
            focus += 1;
        }
        let mut label = if r.tag.is_empty() {
            r.label.clone()
        } else {
            format!("{} · {}", r.label, r.tag)
        };
        // LIVI draws a button row's value after the button: say it in the label.
        let mut value = r.value.clone();
        if k == RowKind::Button && !value.is_empty() {
            label = format!("{label}: {}", std::mem::take(&mut value));
        }
        out.push(Row {
            kind: k,
            node: rows.len(),
            label,
            icon: r.icon,
            value,
            checked: r.checked,
            enabled: true,
            button: r.button.clone(),
            dimmed: r.dimmed,
            alert: r.alert,
            mono: r.mono,
            ..Default::default()
        });
        rows.push(Some(i));
    }
    Native {
        screen: Screen::List {
            title: v.title.clone(),
            rows: out,
            dialog: None,
        },
        rows,
        slots,
    }
}

#[cfg(test)]
mod tests {
    use livi_carpi_ui::view::{Act, Layout, Row as R};

    use super::*;
    use crate::i18n::Strings;
    use crate::settings::{schema, view::Settings};

    #[test]
    fn data_replaces_the_dashboard_and_car_moved_to_settings() {
        let all = vec![
            Tab::Projection,
            Tab::Telemetry,
            Tab::Media,
            Tab::Camera,
            Tab::Settings,
        ];
        assert_eq!(
            tabs(all),
            vec![
                Tab::Projection,
                Tab::Carpi(PageId::Data),
                Tab::Media,
                Tab::Camera,
                Tab::Carpi(PageId::Diagnostics),
                Tab::Settings,
            ]
        );
        assert_eq!(
            tabs(vec![Tab::Projection, Tab::Settings]),
            vec![
                Tab::Projection,
                Tab::Carpi(PageId::Data),
                Tab::Carpi(PageId::Diagnostics),
                Tab::Settings,
            ]
        );
    }

    #[test]
    fn the_tab_is_last_in_settings() {
        let (root, at) = with_tab(schema());
        let n = root.children().len();
        assert_eq!(at, n - 1);
        let s = Settings::new(root);
        assert_eq!(s.find(&[ROUTE]), Some(vec![at]));
        let screen = s
            .screen(&[], &serde_json::Value::Null, &Strings::default())
            .unwrap();
        let Screen::List { rows, .. } = screen else {
            panic!()
        };
        let last = rows.last().unwrap();
        assert_eq!(
            (last.label.as_str(), last.kind),
            ("Car Integration", RowKind::Route)
        );
    }

    #[test]
    fn our_rows_become_livi_rows() {
        let mut v = View::new(Layout::List, "Cluster display", true).note("carpid not connected");
        v.row(
            R::new(Ours::Toggle, "Show on the cluster", "Showing nav")
                .act(Act::Capture)
                .checked(true),
        );
        v.row(R::new(Ours::Info, "Ignition", "On"));
        v.section("hidden");
        v.row(R::new(Ours::Choice, "Units", "Metric").act(Act::Capture));
        v.row(
            R::new(Ours::Info, "Coolant", "90 °C")
                .act(Act::Capture)
                .tag("diag"),
        );
        let at = v.row(
            R::new(Ours::Button, "Capture now", "")
                .button("Capture")
                .act(Act::Capture),
        );
        v.start = at;
        let v = v.finish();
        assert!(!overlay(&v));
        let n = native(&v);
        let Screen::List { rows, title, .. } = &n.screen else {
            panic!()
        };
        assert_eq!(title, "Cluster display");
        let kinds: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Info,
                RowKind::Switch,
                RowKind::Info,
                RowKind::Value,
                RowKind::Pick,
                RowKind::Button
            ]
        );
        assert_eq!(rows[0].label, "carpid not connected");
        assert!(rows[1].checked && rows[1].value == "Showing nav");
        assert_eq!(rows[4].label, "Coolant · diag");
        assert_eq!(rows[5].button, "Capture");
        assert!(rows.iter().enumerate().all(|(i, r)| r.node == i && r.enabled));
        assert_eq!(n.row(0), None);
        assert_eq!(n.row(3), Some(3));
        assert_eq!(n.slots, [Some(0), None, None, Some(1), Some(2), Some(3)]);
        assert_eq!(n.at_slot(1), Some(3));
        assert_eq!(n.slot_of(&v, v.start), Some(3));
    }

    #[test]
    fn a_pick_list_is_a_select() {
        let mut v = View::new(Layout::List, "Time zone", true);
        v.row(
            R::new(Ours::Info, "Denver", "")
                .act(Act::Capture)
                .checked(true),
        );
        v.row(R::new(Ours::Info, "New York", "").act(Act::Capture));
        v.select = true;
        let n = native(&v.finish());
        let Screen::Select { options, .. } = &n.screen else {
            panic!()
        };
        assert!(options[0].selected && !options[1].selected);
        assert_eq!(n.row(1), Some(1));
    }
}
