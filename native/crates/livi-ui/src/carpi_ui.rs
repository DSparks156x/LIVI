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
//!
//! LIVI's own text fields (its Settings' text screens: car name, Wi-Fi password) type on our
//! keyboard too: it opens when such a screen opens and when its field is pushed (keys_ui's
//! StartEditing), and Done commits through LIVI's own `text-commit`, so LIVI still checks the
//! value. With the keyboard Off they work as upstream.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use livi_carpi_ui::view::{Hit, View};
use livi_carpi_ui::{Client, Moved, PageId, Pages, ToastLevel};
use livi_core_proto::state::Front;
use livi_ui_model::carpi::{
    LinkScreen, Native, PHONE_SETTINGS, SettingsRoute, SetupLink, native, overlay, phone_state,
};
use livi_ui_model::settings::node::Node;
use livi_ui_model::settings::view::Screen;
use slint::{ComponentHandle, Timer, TimerMode, Weak};

use crate::{
    AppWindow, CarpiCell, CarpiKey, CarpiKeyRow, CarpiKeyboard, CarpiKind, CarpiLayout, CarpiPill,
    CarpiPoint, CarpiRow, CarpiSpot, CarpiState, CarpiTick, CarpiToast, Focus, FocusZone, Page,
    ScreenKind, SettingsState,
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
    static PHONE_TARGET: RefCell<Option<Vec<usize>>> = const { RefCell::new(None) };
    static SETUP_LINK: RefCell<Option<SetupLink>> = const { RefCell::new(None) };
    /// Settings shows the tab.
    static OWNS: Cell<bool> = const { Cell::new(false) };
    /// Which main Settings card owns the current Car pages, including temporary native visits.
    static ORIGIN: Cell<SettingsRoute> = const { Cell::new(SettingsRoute::Car) };
    /// Our page is up over Settings.
    static OVER: Cell<bool> = const { Cell::new(false) };
    /// The UI path last reported for Settings.
    static REPORTED: Cell<&'static str> = const { Cell::new("") };
    /// The tab's step row being adjusted (our row index): pushed once, the knob turns its value
    /// both ways until a push or Back.
    static TAB_HOLD: Cell<Option<usize>> = const { Cell::new(None) };
    /// LIVI's text screen as last drawn.
    static FIELD: RefCell<Option<Field>> = const { RefCell::new(None) };
}

/// One of LIVI's text screens: its field, and what LIVI accepts in it.
#[derive(Clone, PartialEq)]
struct Field {
    stack: Vec<usize>,
    title: String,
    value: String,
    min: Option<usize>,
    max: Option<usize>,
    url: bool,
    color: bool,
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
    let weak = ui.as_weak();
    ui.global::<SettingsState>().on_color_hex_edit(move || {
        if let Some(ui) = weak.upgrade() {
            start_editing(&ui)
        } else {
            false
        }
    });

    let weak = ui.as_weak();
    ui.global::<CarpiKeyboard>().on_tap(move |r, k| {
        if let Some(ui) = weak.upgrade() {
            keyboard_input(
                &ui,
                livi_carpi_ui::keyboard::Input::Tap(r.max(0) as usize, k.max(0) as usize),
            );
        }
    });

    let timer = Timer::default();
    let weak = ui.as_weak();
    timer.start(TimerMode::Repeated, TICK, move || {
        let Some(ui) = weak.upgrade() else { return };
        let page = ui.get_page();
        let id = id_of(page);
        let tab = page == Page::Settings && OWNS.get();
        let field =
            page == Page::Settings && ui.global::<SettingsState>().get_kind() == ScreenKind::Text;
        let touch = crate::CLIENT.with_borrow(|c| c.as_ref().is_some_and(touch_attached));
        with(|o| {
            ui.global::<CarpiState>()
                .set_flashing_mode(o.client.snapshot().flashing_mode);
            take_toast(&ui, o);
            o.pages.set_touch(touch);
            if id.is_none() {
                o.shown = None;
            }
            let keep = if o.pages.typing_field() {
                field
            } else {
                id.is_some() || tab
            };
            if !keep {
                o.pages.close_keyboard();
            }
            fill_keyboard(&ui.global::<CarpiKeyboard>(), o.pages.keyboard());
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
    if keyboard_key(ui, config, code) {
        return true;
    }
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
        if let Some(row) = TAB_HOLD.get() {
            let turn = if !knob_l.is_empty() && code == knob_l {
                -1
            } else if !knob_r.is_empty() && code == knob_r {
                1
            } else {
                d
            };
            if turn != 0 {
                tab_side_row(ui, row, turn);
                return true;
            }
            let enter = code == "Enter" || code == "NumpadEnter" || code == bind("selectDown");
            let back = code == "Escape" || code == bind("back");
            tab_hold(ui, None);
            // Push and Back end the adjusting; anything else ends it and goes on.
            return enter || back;
        }
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

/// While the on-screen keyboard is up it takes every key: the knob turns through the keys,
/// Up/Down change row, push types, the D-pad moves the cursor, Return deletes, Return held
/// (LIVI's home) closes. A real keyboard's letters, digits and space type. False when it's not
/// up.
fn keyboard_key(ui: &AppWindow, config: &serde_json::Value, code: &str) -> bool {
    use livi_carpi_ui::keyboard::Input as I;
    let Some((open, prefs)) = with(|o| (o.pages.keyboard().is_some(), o.pages.keyboard_prefs()))
    else {
        return false;
    };
    if !open {
        return false;
    }
    // With the keyboard set to Off a real keyboard types, and its H is a letter, not LIVI's
    // home (which is what Return held sends).
    let real = prefs.style == livi_carpi_ui::keyboard::Style::Off;
    let bind = |n: &str| config["bindings"][n].as_str().unwrap_or("").to_string();
    let or = |v: String, fallback: &str| {
        if v.is_empty() {
            fallback.to_string()
        } else {
            v
        }
    };
    let (knob_l, knob_r) = (bind("knobLeft"), bind("knobRight"));
    let (left, right) = (
        or(bind("left"), "ArrowLeft"),
        or(bind("right"), "ArrowRight"),
    );
    let distinct = !knob_l.is_empty() && !knob_r.is_empty() && knob_l != left && knob_r != right;
    let input = if !knob_l.is_empty() && code == knob_l {
        I::Turn(-1)
    } else if !knob_r.is_empty() && code == knob_r {
        I::Turn(1)
    } else if code == left {
        if distinct { I::Left } else { I::Turn(-1) }
    } else if code == right {
        if distinct { I::Right } else { I::Turn(1) }
    } else if code == or(bind("up"), "ArrowUp") {
        I::Up
    } else if code == or(bind("down"), "ArrowDown") {
        I::Down
    } else if code == "Enter" || code == "NumpadEnter" || code == bind("selectDown") {
        I::Push
    } else if code == bind("back") || code == "Backspace" {
        I::Delete
    } else if code == "Escape" || (!real && code == bind("home")) {
        I::Close
    } else if code == "Space" {
        I::Char(' ')
    } else if let Some(c) = code.strip_prefix("Key").filter(|c| c.len() == 1) {
        I::Char(c.to_ascii_lowercase().chars().next().unwrap_or(' '))
    } else if let Some(c) = code.strip_prefix("Digit").filter(|c| c.len() == 1) {
        I::Char(c.chars().next().unwrap_or('0'))
    } else {
        // Anything else is swallowed: nothing under the keyboard reacts.
        return true;
    };
    keyboard_input(ui, input);
    true
}

/// One input to the keyboard; redraws it, and the tab when it closed (its value changed).
fn keyboard_input(ui: &AppWindow, input: livi_carpi_ui::keyboard::Input) {
    let (closed, typed) = with(|o| {
        o.pages.keyboard_input(input, &o.client);
        fill_keyboard(&ui.global::<CarpiKeyboard>(), o.pages.keyboard());
        (o.pages.keyboard().is_none(), o.pages.field_typed())
    })
    .unwrap_or((false, None));
    if let Some(text) = typed {
        field_done(ui, text);
        return;
    }
    if closed {
        if ui.get_page() == Page::Settings && OWNS.get() {
            crate::render_settings(ui);
        } else {
            with(|o| render(ui, o));
        }
    }
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
    let target = livi_ui_model::settings::view::Settings::new(root.clone())
        .find(&PHONE_SETTINGS.split('/').collect::<Vec<_>>());
    PHONE_TARGET.with_borrow_mut(|p| *p = target);
    TAB.set(Some(at));
    root
}

/// Screens the tab has open below its first one (settings_ui's depth).
pub fn settings_depth() -> usize {
    if !OWNS.get() {
        return 0;
    }
    with(|o| ORIGIN.get().depth(&o.pages)).unwrap_or(0)
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
pub fn settings_screen(stack: &[usize], screen: Screen, state: &serde_json::Value) -> Screen {
    text_screen(stack, &screen);
    let resume = SETUP_LINK.with_borrow_mut(|link| {
        let status = link.as_mut()?.screen(stack, TAB.get()?);
        match status {
            LinkScreen::Native => None,
            LinkScreen::Resume(focus) => {
                *link = None;
                Some(focus)
            }
            LinkScreen::Left => {
                *link = None;
                None
            }
        }
    });
    let route = TAB.get().and_then(|t| SettingsRoute::at(stack, t));
    let Some(route) = route else {
        if OWNS.get() {
            leave();
        }
        return if stack.is_empty() {
            livi_ui_model::carpi::root_screen(
                screen,
                with(|o| o.pages.setup_done()).unwrap_or(false),
            )
        } else {
            screen
        };
    };
    let Some(ui) = ui() else { return screen };
    if !OWNS.get() || ORIGIN.get() != route {
        OWNS.set(true);
        ORIGIN.set(route);
        with(|o| {
            if resume.is_none() {
                match route {
                    SettingsRoute::Setup => o.pages.open_setup(&o.client),
                    SettingsRoute::Car => o.pages.reset(PageId::Car),
                }
            }
            live(o, true);
        });
    }
    let built = with(|o| {
        let mut view = o
            .pages
            .view(PageId::Car, &o.client.snapshot(), Instant::now());
        phone_state(&mut view, state);
        take_toast(&ui, o);
        let over = overlay(&view);
        if over {
            show(&ui, o, &view);
        }
        let mut n = native(&view);
        if let (Some(held), Screen::List { rows, .. }) = (TAB_HOLD.get(), &mut n.screen) {
            match n.rows.iter().position(|r| *r == Some(held)) {
                Some(k) if k < rows.len() => {
                    rows[k].value = format!("◀  {}  ▶", rows[k].value);
                }
                _ => TAB_HOLD.set(None),
            }
        }
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
    if let Some(focus) = resume {
        focus_later(&ui, focus, true);
    }
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
    TAB_HOLD.set(None);
    OVER.set(false);
    with(|o| o.tab = None);
    let Some(ui) = ui() else { return };
    ui.global::<CarpiState>().set_over_settings(false);
    if let Some(at) = TAB.get() {
        let done = with(|o| o.pages.setup_done()).unwrap_or(false);
        focus_later(&ui, ORIGIN.get().root_focus(at, done) as i32, true);
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
        let m = f(o, &view, from);
        fill_keyboard(&ui.global::<CarpiKeyboard>(), o.pages.keyboard());
        Some(m)
    })
    .flatten();
    let Some(moved) = moved else { return };
    set_hold(ui, false);
    if with(|o| ORIGIN.get().finished(&o.pages)).unwrap_or(false) {
        // Done leaves the Setup route as well as hiding its card.
        crate::CLIENT.with_borrow(|c| {
            if let Some(c) = c {
                crate::settings_ui::reset_to_root(ui, c);
            }
        });
        return;
    }
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

/// Starts or ends adjusting a step row of the tab, and redraws so its value shows it.
fn tab_hold(ui: &AppWindow, row: Option<usize>) {
    if TAB_HOLD.get() == row {
        return;
    }
    TAB_HOLD.set(row);
    crate::render_settings(ui);
}

/// The knob on the step row being adjusted: one step down or up, staying in adjust mode.
fn tab_side_row(ui: &AppWindow, row: usize, d: i32) {
    let hit = Hit::Row(row);
    let has = with(|o| {
        o.tab
            .as_ref()
            .is_some_and(|(v, _)| v.side_act(Some(&hit), d).is_some())
    })
    .unwrap_or(false);
    if !has {
        return;
    }
    tab_act(ui, |o, view, from| {
        o.pages
            .side(PageId::Car, view, Some(&hit), d, from, &o.client)
    });
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
    // Touch does not necessarily move knob focus. Both inputs must save the pressed row.
    let slot = with(|o| {
        let (view, n) = o.tab.as_ref()?;
        n.slot_of(view, view.rows[row?].focus_at).map(|s| s as i32)
    })
    .flatten()
    .unwrap_or(0);
    let phone = row.is_some_and(|r| {
        with(|o| {
            o.tab
                .as_ref()
                .is_some_and(|(v, _)| v.rows[r].act == livi_carpi_ui::view::Act::PhoneSettings)
        })
        .unwrap_or(false)
    });
    if phone {
        let target = PHONE_TARGET.with_borrow(Clone::clone);
        if let Some(target) = target {
            // Add the current Back/Apply base again after the native visit.
            SETUP_LINK.with_borrow_mut(|link| {
                *link = Some(SetupLink::new(target, ORIGIN.get(), slot));
            });
            crate::CLIENT.with_borrow(|c| {
                if let Some(c) = c {
                    crate::settings_ui::open_path(&ui, c, PHONE_SETTINGS);
                }
            });
            focus_later(&ui, 0, true);
        }
        return true;
    }
    let adjustable = row.is_some_and(|r| {
        with(|o| {
            o.tab.as_ref().is_some_and(|(v, _)| {
                let hit = Hit::Row(r);
                v.steps_only(&hit) && v.side_act(Some(&hit), -1).is_some()
            })
        })
        .unwrap_or(false)
    });
    if adjustable {
        let on = (TAB_HOLD.get() != row).then_some(row).flatten();
        tab_hold(&ui, on);
        return true;
    }
    if let Some(row) = row {
        let focus = ui.global::<SettingsState>().get_focus_base() + slot;
        tab_act(&ui, |o, view, _| {
            o.pages
                .press(PageId::Car, view, &Hit::Row(row), focus, &o.client)
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
    let origin = SETUP_LINK.with_borrow_mut(|link| {
        let link = link.as_mut()?;
        link.back().then(|| link.origin())
    });
    if let Some(origin) = origin {
        if let Some(ui) = weak.upgrade() {
            crate::CLIENT.with_borrow(|c| {
                if let Some(c) = c {
                    crate::settings_ui::open_path(&ui, c, origin.path());
                }
            });
        }
        return true;
    }
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

// ---- LIVI's own text fields ----

/// A touch screen is attached to the main display (LIVI lists them as they come and go).
fn touch_attached(client: &livi_core_client::Client) -> bool {
    livi_ui_model::settings::custom::touch::on_main(&client.mirror().json, 0).is_some()
}

/// settings_screen's look at each screen LIVI draws: a text screen just opened opens the
/// keyboard on its field.
fn text_screen(stack: &[usize], screen: &Screen) {
    let (title, value, min_len, max_len, url, color) = match screen {
        Screen::Text {
            title,
            value,
            min_len,
            max_len,
            url,
        } => (title, value, *min_len, *max_len, *url, false),
        Screen::Color { title, value, .. } => (title, value, None, Some(7), false, true),
        _ => {
            FIELD.with_borrow_mut(|f| *f = None);
            return;
        }
    };
    let field = Field {
        stack: stack.to_vec(),
        title: title.clone(),
        value: value.clone(),
        min: min_len,
        max: max_len,
        url,
        color,
    };
    let opened = FIELD.with_borrow_mut(|f| {
        let opened = f.as_ref().is_none_or(|f| f.stack != field.stack);
        *f = Some(field);
        opened
    });
    if opened && !color {
        // After the render: LIVI's settings are mid-draw here.
        Timer::single_shot(Duration::ZERO, || {
            if let Some(ui) = ui() {
                start_editing(&ui);
            }
        });
    }
}

/// keys_ui's StartEditing: the keyboard on LIVI's text field. False when it isn't one, or the
/// keyboard is Off and LIVI's own field takes a real keyboard.
pub fn start_editing(ui: &AppWindow) -> bool {
    if ui.get_page() != Page::Settings
        || OWNS.get()
        || !matches!(
            ui.global::<SettingsState>().get_kind(),
            ScreenKind::Text | ScreenKind::Color
        )
    {
        return false;
    }
    let Some(f) = FIELD.with_borrow(Clone::clone) else {
        return false;
    };
    let value = if f.color {
        ui.global::<SettingsState>().get_color_hex().to_string()
    } else {
        f.value.clone()
    };
    open_field(ui, &f, &value)
}

fn open_field(ui: &AppWindow, f: &Field, text: &str) -> bool {
    let secret = crate::settings_ui::path().is_some_and(|p| p.to_lowercase().contains("password"));
    let max = f.max.unwrap_or(255);
    with(|o| {
        let open = o.pages.type_field(&f.title, text, secret, max);
        fill_keyboard(&ui.global::<CarpiKeyboard>(), o.pages.keyboard());
        open
    })
    .unwrap_or(false)
}

/// Done on LIVI's field: committed as LIVI's own field commits. When LIVI turns it down (the
/// screen stays), the keyboard comes back with the text and says why.
fn field_done(ui: &AppWindow, text: String) {
    let Some(f) = FIELD.with_borrow(Clone::clone) else {
        return;
    };
    let state = ui.global::<SettingsState>();
    if f.color {
        state.invoke_color_hex_commit(text.as_str().into());
        if !state.get_color_hex_invalid() {
            return;
        }
    } else {
        state.invoke_text_commit(text.as_str().into());
    }
    let still = state.get_kind()
        == if f.color {
            ScreenKind::Color
        } else {
            ScreenKind::Text
        }
        && FIELD.with_borrow(|now| now.as_ref().is_some_and(|n| n.stack == f.stack));
    if !still {
        return;
    }
    let why = if f.color {
        "Not saved: enter a hex color such as #12abef".to_string()
    } else {
        match (f.min, f.max) {
            _ if f.url => "Not saved: it starts with http:// or https://".to_string(),
            (Some(a), Some(b)) => format!("Not saved: {a} to {b} characters"),
            (Some(a), None) => format!("Not saved: at least {a} characters"),
            (None, Some(b)) => format!("Not saved: {b} characters at most"),
            (None, None) => "Not saved".to_string(),
        }
    };
    toast(ui, &why, ToastLevel::Warn);
    open_field(ui, &f, &text);
}
