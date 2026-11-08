// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpi fork: carpid's pages (livi-carpi-ui) in LIVI's shell, and the Car Integration tab in
//! LIVI's Settings. The pages live in their own crate; this is only the Slint glue. lib.rs,
//! keys_ui.rs and settings_ui.rs call in here with one line each, so a rebase onto upstream
//! touches little.
//!
//! Keys: the knob moves focus and push presses, through LIVI's own focus handling. The D-pad
//! (Prev/Next held) changes the focused thing in place, which needs it to send other keys than
//! the knob: LIVI's `knobLeft`/`knobRight` bindings set and different from `left`/`right`. Until
//! then both arrive as ArrowLeft/ArrowRight and move focus, and a step row on our pages is
//! edited by pushing it, turning, and pushing again.
//!
//! The tab: Settings' last route. While its stack is open, settings_ui's render asks
//! [`settings_screen`] for the screen, and its presses come here first. Screens are LIVI rows
//! ([`livi_ui_model::carpi::native`]); the plot and the picker show our own page over Settings
//! (`CarpiState.over-settings`), which takes the keys like one of our pages.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use livi_carpi_ui::view::{Hit, View};
use livi_carpi_ui::{Client, Moved, PageId, Pages, ToastLevel};
use livi_core_proto::state::Front;
use livi_ui_model::carpi::{Native, native, overlay};
use livi_ui_model::settings::node::Node;
use livi_ui_model::settings::view::Screen;
use slint::{ComponentHandle, Timer, TimerMode, Weak};

use crate::{
    AppWindow, CarpiCell, CarpiKind, CarpiLayout, CarpiPill, CarpiPoint, CarpiRow, CarpiSpot,
    CarpiState, CarpiTick, CarpiToast, Focus, FocusZone, Page, SettingsState,
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
    /// carpid is asked for values: one of our pages or the tab is up.
    live: bool,
    view: Option<(PageId, View)>,
    /// The tab's screen as last drawn, and where its rows went.
    tab: Option<(View, Native)>,
    models: Models,
    /// The last of the pages' own toasts shown.
    toast_seq: u64,
}

thread_local! {
    static OURS: RefCell<Option<Ours>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Weak<AppWindow>>> = const { RefCell::new(None) };
    static TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
    static TOAST_HIDE: Timer = Timer::default();
    /// The screen an opened path names below its page (`light` in `/carpi/car/light`).
    static WANT: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The D-pad sends keys of its own.
    static DISTINCT: Cell<bool> = const { Cell::new(false) };
    /// A step row holds the knob's keys.
    static HOLD: Cell<bool> = const { Cell::new(false) };
    /// The tab's index in Settings' root.
    static TAB: Cell<Option<usize>> = const { Cell::new(None) };
    /// Settings shows the tab.
    static OWNS: Cell<bool> = const { Cell::new(false) };
    /// Our page is up over Settings.
    static OVER: Cell<bool> = const { Cell::new(false) };
    /// The UI path last reported for Settings.
    static REPORTED: Cell<&'static str> = const { Cell::new("") };
}

pub fn page_of(id: PageId) -> Page {
    match id {
        PageId::Data => Page::CarpiData,
        PageId::Diagnostics => Page::CarpiDiagnostics,
        PageId::Car => Page::Settings,
    }
}

fn id_of(page: Page) -> Option<PageId> {
    match page {
        Page::CarpiData => Some(PageId::Data),
        Page::CarpiDiagnostics => Some(PageId::Diagnostics),
        _ => None,
    }
}

/// Keys go to our page: one of ours, or ours over Settings.
pub fn ours(page: Page) -> bool {
    id_of(page).is_some() || (page == Page::Settings && OVER.get())
}

/// Our page for a UI path or `LIVI_UI_PAGE` value: `/carpi/data`, `carpi/diagnostics`. The
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
            live(o, true);
        }
        render(ui, o);
        o.pages.path(id)
    });
    (Front::Livi, path.unwrap_or(id.path()))
}

fn with<R>(f: impl FnOnce(&mut Ours) -> R) -> Option<R> {
    OURS.with_borrow_mut(|o| o.as_mut().map(f))
}

fn live(o: &mut Ours, on: bool) {
    if o.live != on {
        o.live = on;
        o.client.shown(on);
    }
}

pub fn wire(ui: &AppWindow) {
    let st = ui.global::<CarpiState>();
    let ours = Ours {
        client: Client::start(),
        pages: Pages::load(),
        shown: None,
        live: false,
        view: None,
        tab: None,
        models: Models::default(),
        toast_seq: 0,
    };
    ours.models.wire(&st);
    OURS.with_borrow_mut(|o| *o = Some(ours));
    UI.with_borrow_mut(|u| *u = Some(ui.as_weak()));

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
        let page = ui.get_page();
        let id = id_of(page);
        let tab = page == Page::Settings && OWNS.get();
        with(|o| {
            if id.is_none() {
                o.shown = None;
            }
            live(o, id.is_some() || tab);
            if id.is_some() {
                render(&ui, o);
            }
        });
        if tab {
            crate::render_settings(&ui);
        }
        if page == Page::Settings {
            let path = settings_path().unwrap_or("/settings");
            if REPORTED.get() != path {
                REPORTED.set(path);
                crate::report_page(&ui);
            }
        }
    });
    TIMER.with_borrow_mut(|t| *t = Some(timer));
}

/// How many things on the shown page take focus.
pub fn targets(ui: &AppWindow) -> usize {
    if ui.get_page() == Page::Settings {
        return with(|o| o.tab.as_ref().map_or(0, |(v, _)| v.targets())).unwrap_or(0);
    }
    with(|o| fresh(ui, o).map_or(0, View::targets)).unwrap_or(0)
}

/// Screens deep below the shown page's first one; 0 when it isn't ours.
pub fn depth(ui: &AppWindow) -> usize {
    let Some(id) = id_of(ui.get_page()) else {
        return 0;
    };
    with(|o| o.pages.depth(id)).unwrap_or(0)
}

/// The view keys act on: the shown page's, or the tab's over Settings.
fn shown_view(ui: &AppWindow, o: &mut Ours) -> Option<View> {
    if ui.get_page() == Page::Settings {
        return o.tab.as_ref().map(|(v, _)| v.clone());
    }
    fresh(ui, o).cloned()
}

/// Push on focus index `at`. A step row without a D-pad of its own starts or ends a hold.
pub fn activate(ui: &AppWindow, at: usize) {
    let Some((hit, steps)) = with(|o| {
        let v = shown_view(ui, o)?;
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

/// Back on one of our pages below its first screen. False leaves it to LIVI, which on Settings
/// comes back through [`settings_back`].
pub fn back(ui: &AppWindow) -> bool {
    if HOLD.get() {
        set_hold(ui, false);
        return true;
    }
    if ui.get_page() == Page::Settings || depth(ui) == 0 {
        return false;
    }
    press(ui, Hit::Back);
    true
}

/// keys_ui's hook, before LIVI decides: the D-pad on our pages and the tab, and the knob while a
/// step row holds it. True when the key was ours.
pub fn key(ui: &AppWindow, config: &serde_json::Value, code: &str) -> bool {
    let main = ui.global::<Focus>().get_zone() == FocusZone::Main;
    let page = ui.get_page();
    let tab = page == Page::Settings && OWNS.get() && !OVER.get();
    if !(ours(page) || tab) || !main {
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
    if tab {
        return distinct && d != 0 && tab_side(ui, d);
    }
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
    if ui.get_page() == Page::Settings {
        tab_act(ui, |o, view, from| {
            o.pages.press(PageId::Car, view, &hit, from, &o.client)
        });
        return;
    }
    act(ui, |o, id, view, from| {
        o.pages.press(id, view, &hit, from, &o.client)
    });
}

/// The D-pad, `d` < 0 left.
fn side(ui: &AppWindow, d: i32) {
    if ui.get_page() == Page::Settings {
        tab_act(ui, |o, view, from| {
            let hit = view.hit(from);
            o.pages
                .side(PageId::Car, view, hit.as_ref(), d, from, &o.client)
        });
        return;
    }
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
        let moved = f(o, id, &view, from);
        render(ui, o);
        let start = o.view.as_ref().map_or(0, |(_, v)| v.start);
        Some((moved, start))
    })
    .flatten();
    let Some((moved, start)) = moved else {
        return;
    };
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

/// Fills `CarpiState` from `view` and raises carpid's toasts.
fn show(ui: &AppWindow, o: &mut Ours, view: &View) {
    o.models.fill(&ui.global::<CarpiState>(), view);
}

fn take_toast(ui: &AppWindow, o: &mut Ours) {
    let raised = o.client.snapshot().toast.clone();
    if let Some(t) = raised.filter(|t| t.seq != o.toast_seq) {
        o.toast_seq = t.seq;
        toast(ui, &t.text, t.level);
    }
}

fn render(ui: &AppWindow, o: &mut Ours) {
    let Some(id) = id_of(ui.get_page()) else {
        return;
    };
    let (view, watch) = {
        let snap = o.client.snapshot();
        (
            o.pages.view(id, &snap, Instant::now()),
            o.pages.watching(id, &snap),
        )
    };
    o.client.watch(watch);
    take_toast(ui, o);
    show(ui, o, &view);
    o.view = Some((id, view));
    let n = o.view.as_ref().map_or(0, |(_, v)| v.targets() as i32);
    let f = ui.global::<Focus>();
    if f.get_zone() == FocusZone::Main && n > 0 && f.get_main_index() >= n {
        f.set_main_index(n - 1);
    }
}

// ---- The Car Integration tab ----

/// Settings' root with the tab last (settings_ui's wire).
pub fn with_tab(root: Node) -> Node {
    let (root, at) = livi_ui_model::carpi::with_tab(root);
    TAB.set(Some(at));
    root
}

/// Screens the tab has open below its first one (settings_ui's depth).
pub fn settings_depth() -> usize {
    if !OWNS.get() {
        return 0;
    }
    with(|o| o.pages.depth(PageId::Car)).unwrap_or(0)
}

/// The UI path while the tab is up: `/carpi/car`, or `/carpi/car/light` on the light screens
/// (carpid's focus `page:light`).
pub fn settings_path() -> Option<&'static str> {
    if !OWNS.get() {
        return None;
    }
    with(|o| o.pages.path(PageId::Car))
}

fn ui() -> Option<AppWindow> {
    UI.with_borrow(|u| u.as_ref().and_then(Weak::upgrade))
}

/// settings_ui's render: the tab's screen when `stack` is the tab, else `screen` as it is.
pub fn settings_screen(stack: &[usize], screen: Screen) -> Screen {
    let ours = TAB.get().is_some_and(|t| stack == [t]);
    if !ours {
        if OWNS.get() {
            leave();
        }
        return screen;
    }
    let Some(ui) = ui() else { return screen };
    if !OWNS.get() {
        OWNS.set(true);
        with(|o| {
            o.pages.reset(PageId::Car);
            live(o, true);
        });
    }
    let built = with(|o| {
        let view = o.pages.view(PageId::Car, &o.client.snapshot(), Instant::now());
        take_toast(&ui, o);
        let over = overlay(&view);
        if over {
            show(&ui, o, &view);
        }
        let n = native(&view);
        let screen = n.screen.clone();
        let title = view.title.clone();
        o.tab = Some((view, n));
        (over, screen, title)
    });
    let Some((over, screen, title)) = built else {
        return screen;
    };
    OVER.set(over);
    ui.global::<CarpiState>().set_over_settings(over);
    if over {
        return Screen::List {
            title,
            rows: Vec::new(),
            dialog: None,
        };
    }
    screen
}

/// The stack left the tab: back on Settings' root, focus on the tab's row.
fn leave() {
    OWNS.set(false);
    OVER.set(false);
    with(|o| o.tab = None);
    let Some(ui) = ui() else { return };
    ui.global::<CarpiState>().set_over_settings(false);
    if let Some(at) = TAB.get() {
        focus_later(&ui, at as i32, true);
    }
}

/// Focus on `at` now and again once LIVI's own key handling has settled; with `base`, `at` is
/// a row of LIVI's list, past its back and apply buttons.
fn focus_later(ui: &AppWindow, at: i32, base: bool) {
    let set = move |ui: &AppWindow| {
        let base = if base {
            ui.global::<SettingsState>().get_focus_base()
        } else {
            0
        };
        let f = ui.global::<Focus>();
        f.set_editing(false);
        if f.get_zone() == FocusZone::Main {
            f.set_main_index(base + at);
        }
    };
    set(ui);
    let weak = ui.as_weak();
    Timer::single_shot(Duration::ZERO, move || {
        if let Some(ui) = weak.upgrade() {
            set(&ui);
        }
    });
}

/// Runs `f` on the tab's screen, redraws Settings, and moves focus as it says.
fn tab_act(ui: &AppWindow, f: impl FnOnce(&mut Ours, &View, i32) -> Moved) {
    let from = ui.global::<Focus>().get_main_index();
    let moved = with(|o| {
        let (view, _) = o.tab.clone()?;
        Some(f(o, &view, from))
    })
    .flatten();
    let Some(moved) = moved else { return };
    set_hold(ui, false);
    crate::render_settings(ui);
    let to = with(|o| {
        let (view, n) = o.tab.as_ref()?;
        let over = OVER.get();
        // Our focus index in the new view, as LIVI's (or our page's) focus index.
        let map = |at: i32| -> Option<(i32, bool)> {
            if over {
                Some((at, false))
            } else {
                n.slot_of(view, at).map(|s| (s as i32, true))
            }
        };
        match moved {
            Moved::Stay => None,
            Moved::Opened => map(view.start).or(Some((0, !over))),
            Moved::Focus(at) => map(at),
            Moved::Closed(at) => Some((at, false)),
        }
    })
    .flatten();
    if let Some((at, base)) = to {
        focus_later(ui, at, base);
    }
}

/// The D-pad on a LIVI row of the tab. False when the row has nothing for it.
fn tab_side(ui: &AppWindow, d: i32) -> bool {
    let at = ui.global::<Focus>().get_main_index() - ui.global::<SettingsState>().get_focus_base();
    let hit = with(|o| {
        let (view, n) = o.tab.as_ref()?;
        let row = n.at_slot(usize::try_from(at).ok()?)?;
        let hit = Hit::Row(row);
        view.side_act(Some(&hit), d).is_some().then_some(hit)
    })
    .flatten();
    let Some(hit) = hit else { return false };
    tab_act(ui, |o, view, from| {
        o.pages
            .side(PageId::Car, view, Some(&hit), d, from, &o.client)
    });
    true
}

/// A push on LIVI's row (or pick) `i` of the tab. False when the tab isn't up.
fn tab_row(weak: &Weak<AppWindow>, i: usize) -> bool {
    if !OWNS.get() {
        return false;
    }
    let Some(ui) = weak.upgrade() else {
        return true;
    };
    let row = with(|o| o.tab.as_ref().and_then(|(_, n)| n.row(i))).flatten();
    if let Some(row) = row {
        tab_act(&ui, |o, view, from| {
            o.pages
                .press(PageId::Car, view, &Hit::Row(row), from, &o.client)
        });
    }
    true
}

/// settings_ui's open, press, toggle and pick, while the tab is up.
pub fn settings_open(weak: &Weak<AppWindow>, node: usize) -> bool {
    tab_row(weak, node)
}

pub fn settings_press(weak: &Weak<AppWindow>, row: usize) -> bool {
    tab_row(weak, row)
}

pub fn settings_toggle(weak: &Weak<AppWindow>, row: usize) -> bool {
    tab_row(weak, row)
}

pub fn settings_pick(weak: &Weak<AppWindow>, pick: usize) -> bool {
    tab_row(weak, pick)
}

/// settings_ui's back: a screen up inside the tab. False on its first screen, where LIVI goes
/// back to Settings' root.
pub fn settings_back(weak: &Weak<AppWindow>) -> bool {
    if settings_depth() == 0 {
        return false;
    }
    let Some(ui) = weak.upgrade() else {
        return true;
    };
    tab_act(&ui, |o, view, from| {
        o.pages
            .press(PageId::Car, view, &Hit::Back, from, &o.client)
    });
    true
}

/// lib.rs's open_path: `/carpi/car[/light|/logging]` opens the tab (carpid's `OpenPage`).
pub fn open_tab(ui: &AppWindow, path: &str) -> bool {
    let Some(rest) = path.trim_start_matches('/').strip_prefix("carpi/car") else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return false;
    }
    let rest = rest.trim_start_matches('/').to_string();
    crate::choose(ui, Page::Settings);
    crate::CLIENT.with_borrow(|c| {
        if let Some(c) = c {
            crate::settings_ui::open_path(ui, c, livi_ui_model::carpi::ROUTE);
        }
    });
    with(|o| o.pages.open(PageId::Car, &rest));
    crate::render_settings(ui);
    let start = with(|o| {
        let (view, n) = o.tab.as_ref()?;
        Some(if OVER.get() {
            (view.start, false)
        } else {
            (n.slot_of(view, view.start).unwrap_or(0) as i32, true)
        })
    })
    .flatten();
    if let Some((at, base)) = start {
        focus_later(ui, at, base);
    }
    true
}
