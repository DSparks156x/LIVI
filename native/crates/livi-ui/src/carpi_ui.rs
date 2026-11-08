// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpi fork: carpid's pages (livi-carpi-ui) in LIVI's shell. The pages live in their own crate;
//! this is only the Slint glue. lib.rs and keys_ui.rs call in here with one line each, so a rebase
//! onto upstream touches little.
//!
//! Keys: the knob moves focus and push presses, through LIVI's own focus handling. The D-pad
//! (Prev/Next held) changes the focused thing in place, which needs it to send other keys than
//! the knob: LIVI's `knobLeft`/`knobRight` bindings set and different from `left`/`right`. Until
//! then both arrive as ArrowLeft/ArrowRight and move focus, and a step row is edited by pushing
//! it, turning, and pushing again.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use livi_carpi_ui::view::{Hit, View};
use livi_carpi_ui::{Client, Moved, PageId, Pages, ToastLevel};
use livi_core_proto::state::Front;
use slint::{ComponentHandle, Timer, TimerMode};

use crate::{
    AppWindow, CarpiCell, CarpiKind, CarpiLayout, CarpiPill, CarpiPoint, CarpiRow, CarpiSpot,
    CarpiState, CarpiTick, CarpiToast, Focus, FocusZone, Page,
};

include!("../../livi-carpi-ui/ui/glue.rs");

/// How often a shown page redraws from carpid's values.
const TICK: Duration = Duration::from_millis(200);
/// How long a toast stays.
const TOAST: Duration = Duration::from_secs(3);

struct Ours {
    client: Client,
    pages: Pages,
    shown: Option<PageId>,
    view: Option<(PageId, View)>,
    models: Models,
    /// The last of the pages' own toasts shown.
    toast_seq: u64,
}

thread_local! {
    static OURS: RefCell<Option<Ours>> = const { RefCell::new(None) };
    static TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
    static TOAST_HIDE: Timer = Timer::default();
    /// The screen an opened path names below its page (`light` in `/carpi/car/light`).
    static WANT: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The D-pad sends keys of its own.
    static DISTINCT: Cell<bool> = const { Cell::new(false) };
    /// A step row holds the knob's keys.
    static HOLD: Cell<bool> = const { Cell::new(false) };
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

/// Our page for a UI path or `LIVI_UI_PAGE` value: `/carpi/data`, `carpi/car/light`, ... The
/// rest of the path picks the screen when the page is reported next.
pub fn named(path: &str) -> Option<Page> {
    let name = path.trim_start_matches('/').strip_prefix("carpi/")?;
    let (first, rest) = name.split_once('/').unwrap_or((name, ""));
    let page = PageId::from_name(first).map(page_of)?;
    WANT.with_borrow_mut(|w| *w = Some(rest.to_string()));
    Some(page)
}

/// What lib.rs reports to the core for one of our pages. A page shown anew starts on its first
/// screen.
pub fn reported(ui: &AppWindow, page: Page) -> (Front, &'static str) {
    let Some(id) = id_of(page) else {
        return (Front::Livi, "/");
    };
    let want = WANT.with_borrow_mut(Option::take);
    set_hold(ui, false);
    let path = with(|o| {
        if let Some(rest) = &want {
            o.pages.open(id, rest);
        } else if o.shown != Some(id) {
            o.pages.reset(id);
        }
        if o.shown != Some(id) {
            o.shown = Some(id);
            o.client.shown(true);
        }
        render(ui, o);
        o.pages.path(id)
    });
    (Front::Livi, path.unwrap_or(id.path()))
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
        models: Models::default(),
        toast_seq: 0,
    };
    ours.models.wire(&st);
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
    let weak = ui.as_weak();
    st.on_cell(move |i| {
        if let Some(ui) = weak.upgrade() {
            press(&ui, Hit::Cell(i.max(0) as usize));
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

/// Push on focus index `at`. A step row without a D-pad of its own starts or ends a hold.
pub fn activate(ui: &AppWindow, at: usize) {
    let Some((hit, steps)) = with(|o| {
        let v = fresh(ui, o)?;
        let hit = v.hit(at as i32)?;
        let steps = v.steps_only(&hit);
        Some((hit, steps))
    })
    .flatten() else {
        return;
    };
    if steps && !DISTINCT.get() {
        set_hold(ui, !HOLD.get());
        return;
    }
    press(ui, hit);
}

/// Back on one of our pages below its first screen. False leaves it to LIVI.
pub fn back(ui: &AppWindow) -> bool {
    if HOLD.get() {
        set_hold(ui, false);
        return true;
    }
    if depth(ui) == 0 {
        return false;
    }
    press(ui, Hit::Back);
    true
}

/// keys_ui's hook, before LIVI decides: the D-pad on our pages, and the knob while a step row
/// holds it. True when the key was ours.
pub fn key(ui: &AppWindow, config: &serde_json::Value, code: &str) -> bool {
    let main = ui.global::<Focus>().get_zone() == FocusZone::Main;
    if !ours(ui.get_page()) || !main {
        HOLD.set(false);
        return false;
    }
    let bind = |n: &str| config["bindings"][n].as_str().unwrap_or("").to_string();
    let or = |v: String, fallback: &str| {
        if v.is_empty() {
            fallback.to_string()
        } else {
            v
        }
    };
    let left = or(bind("left"), "ArrowLeft");
    let right = or(bind("right"), "ArrowRight");
    let (knob_l, knob_r) = (bind("knobLeft"), bind("knobRight"));
    let distinct = !knob_l.is_empty() && !knob_r.is_empty() && knob_l != left && knob_r != right;
    DISTINCT.set(distinct);
    let d = if code == left {
        -1
    } else if code == right {
        1
    } else {
        0
    };
    if HOLD.get() {
        let enter = code == "Enter" || code == "NumpadEnter" || code == bind("selectDown");
        let back = code == "Escape" || code == bind("back");
        if enter || back {
            set_hold(ui, false);
            return true;
        }
        if d != 0 {
            side(ui, d);
            return true;
        }
        return false;
    }
    if distinct && d != 0 {
        side(ui, d);
        return true;
    }
    false
}

fn set_hold(ui: &AppWindow, on: bool) {
    HOLD.set(on);
    ui.global::<CarpiState>().set_holding(on);
}

/// carpid's toasts come over the core's open-page channel as a `/carpi/toast/` path. True when
/// `path` was one.
pub fn toast_path(ui: &AppWindow, path: &str) -> bool {
    match livi_carpi_ui::toast_of(path) {
        Some((text, level)) => {
            toast(ui, &text, level);
            true
        }
        None => false,
    }
}

/// Shows `text` over whatever is on screen for a few seconds.
pub fn toast(ui: &AppWindow, text: &str, level: ToastLevel) {
    let t = ui.global::<CarpiToast>();
    t.set_text(text.into());
    t.set_level(match level {
        ToastLevel::Info => 0,
        ToastLevel::Warn => 1,
        ToastLevel::Error => 2,
    });
    t.set_shown(true);
    let weak = ui.as_weak();
    TOAST_HIDE.with(|timer| {
        timer.start(TimerMode::SingleShot, TOAST, move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<CarpiToast>().set_shown(false);
            }
        })
    });
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
    act(ui, |o, id, view, from| {
        o.pages.press(id, view, &hit, from, &o.client)
    });
}

/// The D-pad, `d` < 0 left.
fn side(ui: &AppWindow, d: i32) {
    act(ui, |o, id, view, from| {
        let hit = view.hit(from);
        o.pages.side(id, view, hit.as_ref(), d, from, &o.client)
    });
}

/// Runs `f` on the shown page, redraws, and moves focus as it says.
fn act(ui: &AppWindow, f: impl FnOnce(&mut Ours, PageId, &View, i32) -> Moved) {
    let Some(id) = id_of(ui.get_page()) else {
        return;
    };
    let focus = ui.global::<Focus>();
    let from = focus.get_main_index();
    let moved = with(|o| {
        let view = fresh(ui, o)?.clone();
        let was = o.pages.path(id);
        let moved = f(o, id, &view, from);
        render(ui, o);
        let start = o.view.as_ref().map_or(0, |(_, v)| v.start);
        Some((moved, start, was != o.pages.path(id)))
    })
    .flatten();
    let Some((moved, start, path_changed)) = moved else {
        return;
    };
    // The light screens have a focus of their own for carpid.
    if path_changed {
        crate::report_page(ui);
    }
    let to = match moved {
        Moved::Stay => return,
        Moved::Opened => start,
        Moved::Closed(at) | Moved::Focus(at) => at,
    };
    set_hold(ui, false);
    focus.set_editing(false);
    focus.set_main_index(to);
    clamp_focus(ui);
}

fn clamp_focus(ui: &AppWindow) {
    let n = with(|o| o.view.as_ref().map_or(0, |(_, v)| v.targets() as i32)).unwrap_or(0);
    let f = ui.global::<Focus>();
    if f.get_zone() == FocusZone::Main && n > 0 && f.get_main_index() >= n {
        f.set_main_index(n - 1);
    }
}

fn render(ui: &AppWindow, o: &mut Ours) {
    let Some(id) = id_of(ui.get_page()) else {
        return;
    };
    let (view, watch, raised) = {
        let snap = o.client.snapshot();
        (
            o.pages.view(id, &snap, Instant::now()),
            o.pages.watching(id, &snap),
            snap.toast.clone(),
        )
    };
    o.client.watch(watch);
    if let Some(t) = raised.filter(|t| t.seq != o.toast_seq) {
        o.toast_seq = t.seq;
        toast(ui, &t.text, t.level);
    }
    o.models.fill(&ui.global::<CarpiState>(), &view);
    o.view = Some((id, view));
    let n = o.view.as_ref().map_or(0, |(_, v)| v.targets() as i32);
    let f = ui.global::<Focus>();
    if f.get_zone() == FocusZone::Main && n > 0 && f.get_main_index() >= n {
        f.set_main_index(n - 1);
    }
}
