// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpi fork: carpid's pages in the tab bar, Setup and Car Integration in Settings.
//!
//! Data takes the dashboard's place. The tab is the last
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
    tabs
}

/// The tab's route key: `/settings/carpi`.
pub const ROUTE: &str = "carpi";

/// A stable route in the cached schema; only its root card is hidden.
pub const SETUP_ROUTE: &str = "setup";
pub const SETUP_NODE: usize = 0;

/// Keep LIVI's native System route and its row in place.
pub const UPDATES_ROUTE: &str = "system/softwareUpdate";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRoute {
    Setup,
    Car,
    Updates,
}

impl SettingsRoute {
    pub fn at(stack: &[usize], tab: usize, updates: Option<&[usize]>) -> Option<Self> {
        if updates == Some(stack) {
            return Some(Self::Updates);
        }
        match stack {
            [SETUP_NODE] => Some(Self::Setup),
            [at] if *at == tab => Some(Self::Car),
            _ => None,
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Self::Setup => SETUP_ROUTE,
            Self::Car => ROUTE,
            Self::Updates => UPDATES_ROUTE,
        }
    }

    /// Setup itself is a root Settings route, rather than a child of Car Integration.
    pub fn depth(self, pages: &livi_carpi_ui::Pages) -> usize {
        pages
            .depth(PageId::Car)
            .saturating_sub(usize::from(self == Self::Setup))
    }

    pub fn finished(self, pages: &livi_carpi_ui::Pages) -> bool {
        self == Self::Setup && pages.screen() == livi_carpi_ui::view::CarScreen::Root
    }

    /// Restore the pressed route's row only when returning to its native parent.
    pub fn parent_focus(
        self,
        stack: &[usize],
        tab: usize,
        setup_done: bool,
        updates: Option<&[usize]>,
    ) -> Option<usize> {
        match self {
            Self::Setup if stack.is_empty() => Some(0),
            Self::Car if stack.is_empty() => Some(tab - usize::from(setup_done)),
            Self::Updates => updates.and_then(|target| {
                let (row, parent) = target.split_last()?;
                (stack == parent).then_some(*row)
            }),
            _ => None,
        }
    }
}

/// Read the current preference on every render, without changing native route indexes.
pub fn root_screen(mut screen: Screen, setup_done: bool) -> Screen {
    if let Screen::List { rows, .. } = &mut screen {
        if setup_done {
            rows.retain(|r| r.node != SETUP_NODE);
        } else if let Some(row) = rows.iter_mut().find(|r| r.node == SETUP_NODE) {
            row.value = "Start here".into();
        }
    }
    screen
}

/// LIVI owns the controls, persistence, Apply/restart and hardware behavior on this route.
pub const PHONE_SETTINGS: &str = "general/connections";

/// A temporary visit from Setup to an existing Settings route, including its children.
pub struct SetupLink {
    target: Vec<usize>,
    origin: SettingsRoute,
    focus: i32,
    at_target: bool,
    returning: bool,
}

#[derive(Debug, PartialEq)]
pub enum LinkScreen {
    Native,
    Resume(i32),
    Left,
}

impl SetupLink {
    pub fn new(target: Vec<usize>, origin: SettingsRoute, focus: i32) -> Self {
        Self {
            target,
            origin,
            focus,
            at_target: false,
            returning: false,
        }
    }

    pub fn screen(&mut self, stack: &[usize], tab: usize) -> LinkScreen {
        if self.returning && SettingsRoute::at(stack, tab, None) == Some(self.origin) {
            return LinkScreen::Resume(self.focus);
        }
        self.at_target = stack == self.target;
        if stack.starts_with(&self.target) {
            LinkScreen::Native
        } else {
            LinkScreen::Left
        }
    }

    /// Nested screens keep their normal Back; only the target returns to Setup.
    pub fn back(&mut self) -> bool {
        self.returning = self.at_target;
        self.returning
    }

    pub fn origin(&self) -> SettingsRoute {
        self.origin
    }
}

/// Show configured state, without claiming that a phone or authentication hardware is ready.
pub fn phone_state(view: &mut View, state: &serde_json::Value) {
    use livi_carpi_ui::view::Act;
    let Some(row) = view.rows.iter_mut().find(|r| r.act == Act::PhoneSettings) else {
        return;
    };
    let c = &state["config"];
    if let (Some(aa), Some(cp)) = (
        c["wirelessAaEnabled"].as_bool(),
        c["wirelessCpEnabled"].as_bool(),
    ) {
        row.value = format!(
            "{}Android Auto {} · CarPlay {}",
            if aa || cp { "" } else { "To do · " },
            if aa { "on" } else { "off" },
            if cp { "on" } else { "off" },
        );
    }
}

/// Settings' root with Setup first and Car Integration last, and the latter's index.
pub fn with_tab(mut root: Node) -> (Node, usize) {
    // Replace only the contents of the native slot. Its custom child would otherwise
    // trigger LIVI's CheckUpdate watch before our screen is rendered.
    if let Kind::Route { children, .. } = &mut root.kind
        && let Some(system) = children
            .iter_mut()
            .find(|n| n.route_key() == Some("system"))
        && let Kind::Route { children, .. } = &mut system.kind
        && let Some(update) = children
            .iter_mut()
            .find(|n| n.route_key() == Some("softwareUpdate"))
        && let Kind::Route { children, .. } = &mut update.kind
    {
        children.clear();
    }
    let tab = Node::route(ROUTE, Label::plain("Car Integration"), vec![]).icon("carName");
    let mut at = 0;
    if let Kind::Route { children, .. } = &mut root.kind {
        children.insert(
            SETUP_NODE,
            Node::route(SETUP_ROUTE, Label::plain("Setup"), vec![]).icon("dabStationList"),
        );
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
    use std::time::Instant;

    use super::*;
    use crate::i18n::Strings;
    use crate::settings::{schema, view::Settings};

    #[test]
    fn updates_replaces_only_native_slot_contents_and_disables_upstream_watches() {
        use crate::settings::custom::Watch;
        use livi_core_proto::message::Action;

        let upstream = Settings::new(schema());
        let keys = UPDATES_ROUTE.split('/').collect::<Vec<_>>();
        let old_target = upstream.find(&keys).unwrap();
        assert!(
            upstream
                .watches(&old_target)
                .iter()
                .any(|w| { matches!(w, Watch::Act(Action::CheckUpdate)) })
        );
        let (root, tab) = with_tab(schema());
        let settings = Settings::new(root);
        let target = settings.find(&keys).unwrap();
        assert_eq!(target.len(), 2);
        assert_eq!(target[1], old_target[1]);
        let mut expected = upstream.node(&old_target).unwrap().clone();
        let Kind::Route { children, .. } = &mut expected.kind else {
            panic!()
        };
        children.clear();
        assert_eq!(settings.node(&target), Some(&expected));
        assert!(settings.watches(&target).is_empty());
        assert!(settings.node(&[target.clone(), vec![0]].concat()).is_none());
        let Screen::List { rows, dialog, .. } = settings
            .screen(&target, &serde_json::Value::Null, &Strings::default())
            .unwrap()
        else {
            panic!()
        };
        assert!(rows.is_empty() && dialog.is_none());
        // The System page still has the same rows, labels, icons and native node indexes.
        assert_eq!(
            settings.screen(&target[..1], &serde_json::Value::Null, &Strings::default()),
            upstream.screen(
                &old_target[..1],
                &serde_json::Value::Null,
                &Strings::default()
            )
        );
        assert!(settings.opens(&target[..1], target[1]));
        assert_eq!(
            SettingsRoute::at(&target, tab, Some(&target)),
            Some(SettingsRoute::Updates)
        );
        for stack in [vec![], target[..1].to_vec(), vec![target[0], target[1] + 1]] {
            assert_eq!(SettingsRoute::at(&stack, tab, Some(&target)), None);
        }
        assert_eq!(SettingsRoute::Updates.path(), UPDATES_ROUTE);
        assert_eq!(
            SettingsRoute::Updates.parent_focus(&target[..1], tab, true, Some(&target)),
            Some(target[1])
        );
        assert_eq!(
            SettingsRoute::Updates.parent_focus(&[], tab, true, Some(&target)),
            None
        );
    }

    #[test]
    fn updates_native_touch_and_knob_rows_return_focus_and_depth_to_the_slot() {
        use livi_carpi_ui::view::{CarScreen, Hit};
        use livi_carpi_ui::{Client, Moved, Pages};

        let (root, tab) = with_tab(schema());
        let settings = Settings::new(root);
        let target = settings
            .find(&UPDATES_ROUTE.split('/').collect::<Vec<_>>())
            .unwrap();
        let client = Client::offline("test");
        let mut pages = Pages::new(Default::default(), None);
        let mut snap = livi_carpi_ui::client::Snapshot::default();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../livi-carpi-ui/fixtures/car.json")).unwrap();
        snap.settings = Some(serde_json::from_value(fixture["settings"].clone()).unwrap());
        let label = "Wi-Fi networks";
        pages.open_updates(&client);
        assert_eq!(target.len() + SettingsRoute::Updates.depth(&pages), 2);
        let view = pages.view(PageId::Car, &snap, Instant::now());
        let row = view.rows.iter().position(|r| r.label == label).unwrap();
        let mapped = native(&view);
        let touch = mapped.rows.iter().position(|r| *r == Some(row)).unwrap();
        let slot = mapped.slot_of(&view, view.rows[row].focus_at).unwrap();
        assert_eq!(mapped.row(touch), mapped.at_slot(slot));
        let focus = slot as i32 + 1; // Native Back button precedes the rows.
        assert_eq!(
            pages.press(PageId::Car, &view, &Hit::Row(row), focus, &client),
            Moved::Opened
        );
        assert_eq!(target.len() + SettingsRoute::Updates.depth(&pages), 3);
        assert_eq!(pages.path(PageId::Car), livi_carpi_ui::pages::UPDATES_PATH);
        assert_eq!(pages.back(PageId::Car), Some(Moved::Closed(focus)));
        assert_eq!(pages.screen(), CarScreen::Updates);
        assert_eq!(SettingsRoute::Updates.depth(&pages), 0);
        assert_eq!(pages.back(PageId::Car), None); // LIVI pops to System.
        assert_eq!(
            SettingsRoute::Updates.parent_focus(&target[..1], tab, false, Some(&target)),
            Some(target[1])
        );
    }

    #[test]
    fn setup_phone_link_uses_native_settings_and_returns_with_current_state() {
        use crate::settings::{restart::pending, view::Change};
        use livi_carpi_ui::{Client, Pages};
        use serde_json::json;
        use std::time::Instant;

        let (root, tab) = with_tab(schema());
        let settings = Settings::new(root);
        let target = settings
            .find(&PHONE_SETTINGS.split('/').collect::<Vec<_>>())
            .unwrap();
        let client = Client::offline("test");
        let mut pages = Pages::new(Default::default(), None);
        let snap = livi_carpi_ui::client::Snapshot::default();
        pages.open_setup(&client);
        let mut view = pages.view(PageId::Car, &snap, Instant::now());
        let mut state = json!({"config": {"wirelessAaEnabled": false, "wirelessCpEnabled": false}});
        phone_state(&mut view, &state);
        let phone = view
            .rows
            .iter()
            .position(|r| r.act == Act::PhoneSettings)
            .unwrap();
        assert!(view.rows[phone].value.starts_with("To do"));
        let native = native(&view);
        // Touch's node index and the knob's focus slot reach the same route.
        let touch = native.rows.iter().position(|r| *r == Some(phone)).unwrap();
        assert_eq!(native.row(touch), Some(phone));
        let slot = native.slot_of(&view, view.rows[phone].focus_at).unwrap();
        assert_eq!(native.at_slot(slot), Some(phone));

        let mut link = SetupLink::new(target.clone(), SettingsRoute::Setup, slot as i32);
        assert_eq!(link.origin().path(), SETUP_ROUTE);
        assert_eq!(link.screen(&target, tab), LinkScreen::Native);
        for (key, other) in [
            ("wirelessAaEnabled", "wirelessCpEnabled"),
            ("wirelessCpEnabled", "wirelessAaEnabled"),
        ] {
            let Screen::List { rows, .. } = settings
                .screen(&target, &state, &Strings::default())
                .unwrap()
            else {
                panic!()
            };
            let node = rows
                .iter()
                .find(|r| {
                    settings
                        .node(&[target.clone(), vec![r.node]].concat())
                        .unwrap()
                        .path
                        == key
                })
                .unwrap()
                .node;
            let Change::Config(patch) = settings.toggle(&target, node, &state).unwrap() else {
                panic!()
            };
            assert_eq!(patch, json!({key: true}));
            assert!(patch.get(other).is_none());
            let baseline = state["config"].clone();
            state["config"][key] = patch[key].clone();
            assert!(pending(&state["config"], &baseline));
            let Screen::List { rows, .. } = settings
                .screen(&target, &state, &Strings::default())
                .unwrap()
            else {
                panic!()
            };
            assert!(rows.iter().find(|r| r.node == node).unwrap().checked);
        }
        let mut child = target.clone();
        child.push(0);
        assert_eq!(link.screen(&child, tab), LinkScreen::Native);
        assert!(!link.back());
        assert_eq!(link.screen(&target, tab), LinkScreen::Native);
        assert!(link.back());
        assert_eq!(
            link.screen(&[SETUP_NODE], tab),
            LinkScreen::Resume(slot as i32)
        );
        let mut view = pages.view(PageId::Car, &snap, Instant::now());
        phone_state(&mut view, &state);
        assert_eq!(view.title, "Setup");
        assert_eq!(view.rows[phone].value, "Android Auto on · CarPlay on");
        // The native Settings stack pops this root route; no Car Integration screen is opened.
        assert_eq!(SettingsRoute::Setup.depth(&pages), 0);
    }

    #[test]
    fn phone_link_cancels_when_an_unrelated_route_opens() {
        let mut link = SetupLink::new(vec![1, 0], SettingsRoute::Setup, 3);
        assert_eq!(link.screen(&[1, 0], 7), LinkScreen::Native);
        assert_eq!(link.screen(&[], 7), LinkScreen::Left);
        assert!(!link.back());
    }

    #[test]
    fn phone_summary_distinguishes_each_setting_and_unknown_state() {
        use serde_json::json;
        for (aa, cp, expected) in [
            (false, false, "To do · Android Auto off · CarPlay off"),
            (true, false, "Android Auto on · CarPlay off"),
            (false, true, "Android Auto off · CarPlay on"),
            (true, true, "Android Auto on · CarPlay on"),
        ] {
            let mut v = View::new(Layout::List, "Setup", true);
            v.row(
                R::new(
                    Ours::Route,
                    "Phone connection",
                    "Configure wireless Android Auto / CarPlay",
                )
                .act(Act::PhoneSettings),
            );
            phone_state(
                &mut v,
                &json!({"config": {"wirelessAaEnabled": aa, "wirelessCpEnabled": cp}}),
            );
            assert_eq!(v.rows[0].value, expected);
        }
        let mut v = View::new(Layout::List, "Setup", true);
        v.row(
            R::new(
                Ours::Route,
                "Phone connection",
                "Configure wireless Android Auto / CarPlay",
            )
            .act(Act::PhoneSettings),
        );
        phone_state(&mut v, &json!({}));
        assert_eq!(v.rows[0].value, "Configure wireless Android Auto / CarPlay");
    }

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
                Tab::Settings,
            ]
        );
        assert_eq!(
            tabs(vec![Tab::Projection, Tab::Settings]),
            vec![
                Tab::Projection,
                Tab::Carpi(PageId::Data),
                Tab::Settings,
            ]
        );
    }

    #[test]
    fn setup_is_first_and_car_integration_is_last_in_settings() {
        let (root, at) = with_tab(schema());
        let n = root.children().len();
        assert_eq!(at, n - 1);
        let s = Settings::new(root);
        assert_eq!(s.find(&[SETUP_ROUTE]), Some(vec![SETUP_NODE]));
        assert_eq!(s.find(&[ROUTE]), Some(vec![at]));
        let screen = s
            .screen(&[], &serde_json::Value::Null, &Strings::default())
            .unwrap();
        let Screen::List { rows, .. } = screen else {
            panic!()
        };
        assert_eq!(
            (rows[0].label.as_str(), rows[0].kind, rows[0].node),
            ("Setup", RowKind::Route, SETUP_NODE)
        );
        let last = rows.last().unwrap();
        assert_eq!(
            (last.label.as_str(), last.kind),
            ("Car Integration", RowKind::Route)
        );
    }

    #[test]
    fn hide_and_restore_update_the_cached_root_and_preserve_preferences() {
        use livi_carpi_ui::view::{CarScreen, Hit};
        use livi_carpi_ui::{
            Client, Pages,
            config::{self, Saved},
        };
        let (root, tab) = with_tab(schema());
        let settings = Settings::new(root);
        let cached = settings
            .screen(&[], &serde_json::Value::Null, &Strings::default())
            .unwrap();
        let file = std::env::temp_dir().join(format!(
            "livi-setup-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let saved = Saved {
            recent: vec!["engine.rpm".into()],
            data: Some(livi_carpi_ui::slots::Board::data()),
            ..Saved::default()
        };
        config::save(&file, &saved).unwrap();
        let client = Client::offline("test");
        let mut pages = Pages::new(config::load(&file), Some(file.clone()));
        let mut snap = livi_carpi_ui::client::Snapshot::default();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../livi-carpi-ui/fixtures/car.json")).unwrap();
        snap.settings = Some(serde_json::from_value(fixture["settings"].clone()).unwrap());
        let root_rows = |done| {
            let Screen::List { rows, .. } = root_screen(cached.clone(), done) else {
                panic!()
            };
            rows
        };
        assert_eq!(root_rows(pages.setup_done())[0].label, "Setup");
        pages.open_setup(&client);
        assert_eq!(SettingsRoute::Setup.depth(&pages), 0);
        let view = pages.view(PageId::Car, &snap, Instant::now());
        let hide = view
            .rows
            .iter()
            .position(|r| r.act == Act::SetupDone(true))
            .unwrap();
        let mapped = native(&view);
        // Touch and knob resolve the same hide action.
        assert_eq!(
            mapped.row(mapped.rows.iter().position(|r| *r == Some(hide)).unwrap()),
            Some(hide)
        );
        assert_eq!(
            mapped.at_slot(mapped.slot_of(&view, view.rows[hide].focus_at).unwrap()),
            Some(hide)
        );
        pages.press(PageId::Car, &view, &Hit::Row(hide), 0, &client);
        assert!(SettingsRoute::Setup.finished(&pages));
        assert!(pages.setup_done());
        let hidden = root_rows(pages.setup_done());
        assert!(hidden.iter().all(|r| r.label != "Setup"));
        // Row positions move; native node keys and the Connections target stay stable.
        assert_eq!(hidden[0].node, 1);
        assert_eq!(hidden.last().unwrap().node, tab);
        assert_eq!(
            SettingsRoute::Car.parent_focus(&[], tab, true, None),
            Some(hidden.len() - 1)
        );
        assert!(settings.opens(&[], hidden[0].node));
        let loaded = config::load(&file);
        assert!(loaded.setup_done);
        assert_eq!(loaded.recent, saved.recent);
        assert_eq!(loaded.panels(), saved.panels());
        assert_eq!(loaded.keyboard, saved.keyboard);
        pages = Pages::new(loaded, Some(file.clone()));
        assert!(
            root_rows(pages.setup_done())
                .iter()
                .all(|r| r.node != SETUP_NODE)
        );
        pages.goto(CarScreen::Advanced);
        let view = pages.view(PageId::Car, &snap, Instant::now());
        let show = view
            .rows
            .iter()
            .position(|r| r.act == Act::SetupDone(false))
            .unwrap();
        pages.press(PageId::Car, &view, &Hit::Row(show), 0, &client);
        assert_eq!(pages.screen(), CarScreen::Advanced);
        assert!(!pages.setup_done());
        assert!(!config::load(&file).setup_done);
        let shown = root_rows(pages.setup_done());
        assert_eq!(
            (shown[0].label.as_str(), shown[0].value.as_str()),
            ("Setup", "Start here")
        );
        assert_eq!(shown.last().unwrap().node, tab);
        assert_eq!(
            SettingsRoute::Car.parent_focus(&[], tab, false, None),
            Some(shown.len() - 1)
        );
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn setup_routes_and_nested_back_keep_their_origin_and_focus() {
        use livi_carpi_ui::view::{CarScreen, Hit};
        use livi_carpi_ui::{Client, Moved, Pages};
        let (_, tab) = with_tab(schema());
        assert_eq!(
            SettingsRoute::at(&[SETUP_NODE], tab, None),
            Some(SettingsRoute::Setup)
        );
        assert_eq!(
            SettingsRoute::at(&[tab], tab, None),
            Some(SettingsRoute::Car)
        );
        assert_eq!(SettingsRoute::at(&[], tab, None), None);
        let client = Client::offline("test");
        let mut pages = Pages::new(Default::default(), None);
        let mut snap = livi_carpi_ui::client::Snapshot::default();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../livi-carpi-ui/fixtures/car.json")).unwrap();
        snap.settings = Some(serde_json::from_value(fixture["settings"].clone()).unwrap());
        for label in [
            "Time zone",
            "Wi-Fi networks",
            "Cluster display",
            "Remote access",
        ] {
            pages.open_setup(&client);
            let view = pages.view(PageId::Car, &snap, Instant::now());
            let row = view.rows.iter().position(|r| r.label == label).unwrap();
            let mapped = native(&view);
            let slot = mapped.slot_of(&view, view.rows[row].focus_at).unwrap();
            assert_eq!(mapped.at_slot(slot), Some(row));
            let focus = slot as i32 + 1;
            pages.press(PageId::Car, &view, &Hit::Row(row), focus, &client);
            assert_eq!(SettingsRoute::Setup.depth(&pages), 1);
            assert_eq!(pages.back(PageId::Car), Some(Moved::Closed(focus)));
            assert_eq!(pages.screen(), CarScreen::Setup);
            assert_eq!(SettingsRoute::Setup.depth(&pages), 0);
        }
        pages.reset(PageId::Car);
        assert_eq!(SettingsRoute::Car.depth(&pages), 0);
        assert!(
            !pages
                .view(PageId::Car, &snap, Instant::now())
                .rows
                .iter()
                .any(|r| r.act == Act::Car(CarScreen::Setup))
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
        assert!(
            rows.iter()
                .enumerate()
                .all(|(i, r)| r.node == i && r.enabled)
        );
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
