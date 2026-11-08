// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpi fork: carpid's pages (livi-carpi-ui) in LIVI's shell. The pages live in their own crate;
//! this is only the Slint glue. lib.rs and keys_ui.rs call in here with one line each, so a rebase
//! onto upstream touches little.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use livi_carpi_ui::view::{Hit, Layout, Row, RowKind as Kind, View};
use livi_carpi_ui::{Client, Moved, PageId, Pages};
use livi_core_proto::state::Front;
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::{
    AppWindow, CarpiCell, CarpiLayout, CarpiPill, CarpiRow, CarpiState, Focus, FocusZone, Page,
    RowData, RowKind,
};

/// How often a shown page redraws from carpid's values.
const TICK: Duration = Duration::from_millis(200);

struct Ours {
    client: Client,
    pages: Pages,
    shown: Option<PageId>,
    view: Option<(PageId, View)>,
    pills: Rc<VecModel<CarpiPill>>,
    rows: Rc<VecModel<CarpiRow>>,
    cells: Rc<VecModel<CarpiCell>>,
    scroll_rows: Rc<VecModel<i32>>,
    scroll_sections: Rc<VecModel<i32>>,
    no_tools: ModelRc<crate::ToolData>,
}

thread_local! {
    static OURS: RefCell<Option<Ours>> = const { RefCell::new(None) };
    static TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

pub fn page_of(id: PageId) -> Page {
    match id {
        PageId::Data => Page::CarpiData,
        PageId::Diagnostics => Page::CarpiDiagnostics,
        PageId::Car => Page::CarpiCar,
    }
}

fn id_of(page: Page) -> Option<PageId> {
    match page {
        Page::CarpiData => Some(PageId::Data),
        Page::CarpiDiagnostics => Some(PageId::Diagnostics),
        Page::CarpiCar => Some(PageId::Car),
        _ => None,
    }
}

pub fn ours(page: Page) -> bool {
    id_of(page).is_some()
}

/// Our page for a UI path or `LIVI_UI_PAGE` value: `/carpi/data`, `carpi/car`, ...
pub fn named(path: &str) -> Option<Page> {
    let name = path.trim_start_matches('/').strip_prefix("carpi/")?;
    PageId::from_name(name.split('/').next()?).map(page_of)
}

/// What lib.rs reports to the core for one of our pages. A page shown anew starts on its first
/// screen.
pub fn reported(ui: &AppWindow, page: Page) -> (Front, &'static str) {
    let Some(id) = id_of(page) else {
        return (Front::Livi, "/");
    };
    with(|o| {
        if o.shown != Some(id) {
            o.pages.reset(id);
            o.shown = Some(id);
            o.client.shown(true);
        }
        render(ui, o);
    });
    (Front::Livi, id.path())
}

fn with<R>(f: impl FnOnce(&mut Ours) -> R) -> Option<R> {
    OURS.with_borrow_mut(|o| o.as_mut().map(f))
}

pub fn wire(ui: &AppWindow) {
    let st = ui.global::<CarpiState>();
    let ours = Ours {
        client: Client::start(),
        pages: Pages::load(),
        shown: None,
        view: None,
        pills: Rc::new(VecModel::default()),
        rows: Rc::new(VecModel::default()),
        cells: Rc::new(VecModel::default()),
        scroll_rows: Rc::new(VecModel::default()),
        scroll_sections: Rc::new(VecModel::default()),
        no_tools: ModelRc::from(Rc::new(VecModel::<crate::ToolData>::default())),
    };
    st.set_pills(ModelRc::from(ours.pills.clone()));
    st.set_rows(ModelRc::from(ours.rows.clone()));
    st.set_cells(ModelRc::from(ours.cells.clone()));
    st.set_scroll_rows(ModelRc::from(ours.scroll_rows.clone()));
    st.set_scroll_sections(ModelRc::from(ours.scroll_sections.clone()));
    OURS.with_borrow_mut(|o| *o = Some(ours));

    let weak = ui.as_weak();
    st.on_back(move || {
        if let Some(ui) = weak.upgrade() {
            press(&ui, Hit::Back);
        }
    });
    let weak = ui.as_weak();
    st.on_pill(move |i| {
        if let Some(ui) = weak.upgrade() {
            press(&ui, Hit::Pill(i.max(0) as usize));
        }
    });
    let weak = ui.as_weak();
    st.on_row(move |i| {
        if let Some(ui) = weak.upgrade() {
            press(&ui, Hit::Row(i.max(0) as usize));
        }
    });

    let timer = Timer::default();
    let weak = ui.as_weak();
    timer.start(TimerMode::Repeated, TICK, move || {
        let Some(ui) = weak.upgrade() else { return };
        let id = id_of(ui.get_page());
        with(|o| {
            if id.is_none() && o.shown.is_some() {
                o.shown = None;
                o.client.shown(false);
            }
            if id.is_some() {
                render(&ui, o);
            }
        });
    });
    TIMER.with_borrow_mut(|t| *t = Some(timer));
}

/// How many things on the shown page take focus.
pub fn targets(ui: &AppWindow) -> usize {
    with(|o| fresh(ui, o).map_or(0, View::targets)).unwrap_or(0)
}

/// Screens deep below the shown page's first one; 0 when it isn't ours.
pub fn depth(ui: &AppWindow) -> usize {
    let Some(id) = id_of(ui.get_page()) else {
        return 0;
    };
    with(|o| o.pages.depth(id)).unwrap_or(0)
}

/// Select on focus index `at`.
pub fn activate(ui: &AppWindow, at: usize) {
    let hit = with(|o| fresh(ui, o).and_then(|v| v.hit(at as i32))).flatten();
    if let Some(hit) = hit {
        press(ui, hit);
    }
}

/// Back on one of our pages below its first screen. False leaves it to LIVI.
pub fn back(ui: &AppWindow) -> bool {
    if depth(ui) == 0 {
        return false;
    }
    press(ui, Hit::Back);
    true
}

/// The view of the shown page, drawn now if the last one was of another page.
fn fresh<'a>(ui: &AppWindow, o: &'a mut Ours) -> Option<&'a View> {
    let id = id_of(ui.get_page())?;
    if o.view.as_ref().is_none_or(|(at, _)| *at != id) {
        render(ui, o);
    }
    o.view.as_ref().map(|(_, v)| v)
}

fn press(ui: &AppWindow, hit: Hit) {
    let Some(id) = id_of(ui.get_page()) else {
        return;
    };
    let focus = ui.global::<Focus>();
    let from = focus.get_main_index();
    let moved = with(|o| {
        let view = fresh(ui, o)?.clone();
        let moved = o.pages.press(id, &view, &hit, from, &o.client);
        render(ui, o);
        let first = o.view.as_ref().map_or(0, |(_, v)| v.first_row());
        Some((moved, first))
    })
    .flatten();
    match moved {
        Some((Moved::Opened, first)) => {
            focus.set_editing(false);
            focus.set_main_index(first);
        }
        Some((Moved::Closed(at), _)) => {
            focus.set_editing(false);
            focus.set_main_index(at);
        }
        _ => {}
    }
}

fn render(ui: &AppWindow, o: &mut Ours) {
    let Some(id) = id_of(ui.get_page()) else {
        return;
    };
    let view = {
        let snap = o.client.snapshot();
        o.pages.view(id, &snap, Instant::now())
    };
    let st = ui.global::<CarpiState>();
    st.set_layout(match view.layout {
        Layout::List => CarpiLayout::List,
        Layout::Grid => CarpiLayout::Grid,
    });
    st.set_title(view.title.as_str().into());
    st.set_note(view.note.as_str().into());
    st.set_back_at(view.back_at);
    let base = view
        .rows
        .iter()
        .find(|r| r.focus_at >= 0)
        .map_or(view.targets() as i32, |r| r.focus_at);
    st.set_row_base(base);
    sync(
        &o.pills,
        view.pills
            .iter()
            .map(|p| CarpiPill {
                label: p.label.as_str().into(),
                active: p.active,
                enabled: p.enabled,
                focus_at: p.focus_at,
            })
            .collect(),
    );
    sync(
        &o.rows,
        view.rows.iter().map(|r| row(r, &o.no_tools)).collect(),
    );
    sync(
        &o.cells,
        view.cells
            .iter()
            .map(|c| CarpiCell {
                label: c.label.as_str().into(),
                name: c.name.as_str().into(),
                value: c.value.as_str().into(),
                unit: c.unit.as_str().into(),
                age: c.age.as_str().into(),
                dim: c.dim,
            })
            .collect(),
    );
    sync(&o.scroll_rows, view.scroll.iter().map(|s| s.0).collect());
    sync(
        &o.scroll_sections,
        view.scroll.iter().map(|s| s.1).collect(),
    );
    let f = ui.global::<Focus>();
    let n = view.targets() as i32;
    if f.get_zone() == FocusZone::Main && n > 0 && f.get_main_index() >= n {
        f.set_main_index(n - 1);
    }
    o.view = Some((id, view));
}

fn row(r: &Row, no_tools: &ModelRc<crate::ToolData>) -> CarpiRow {
    let kind = match r.kind {
        Kind::Section | Kind::Info => RowKind::Info,
        Kind::Route => RowKind::Route,
        Kind::Button => RowKind::Button,
        Kind::Toggle => RowKind::Toggle,
    };
    CarpiRow {
        section: r.kind == Kind::Section,
        title: r.label.as_str().into(),
        first: r.first,
        last: r.last,
        data: RowData {
            kind,
            label: r.label.as_str().into(),
            value: r.value.as_str().into(),
            button: r.button.as_str().into(),
            enabled: r.enabled,
            dimmed: r.dimmed,
            alert: r.alert,
            checked: r.checked,
            mono: r.mono,
            focus_at: r.focus_at,
            tool_offset: -1,
            tools: no_tools.clone(),
            ..Default::default()
        },
    }
}

/// Updates `model` in place, so rows that didn't change keep their state.
fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, items: Vec<T>) {
    let n = items.len();
    for (i, item) in items.into_iter().enumerate() {
        if i >= model.row_count() {
            model.push(item);
        } else if model.row_data(i).as_ref() != Some(&item) {
            model.set_row_data(i, item);
        }
    }
    while model.row_count() > n {
        model.remove(model.row_count() - 1);
    }
}
