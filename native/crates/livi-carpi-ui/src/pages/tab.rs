// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Car Integration tab in LIVI's Settings (docs/PAGES.md 5a): every screen of the tree, and
//! the acts only it has. Grouped by what you're doing; live state sits next to the setting it
//! explains.

use std::time::{Duration, Instant};

use carpi_proto::{
    AvsdPatch, AvsdRange, ClockPatch, DarkModeSource, DisClaim, DisClock, DisLanes, DisMediaLayout,
    DisNavStyle, DisPage, DisPageSetting, DisPatch, DisUnits, InputPatch, LightAction, LightPatch,
    LogsPatch, PinChange, PortalPatch, PowerPatch, Settings, SettingsPatch, UpdateAction,
    UpdateChannel, UpdatePatch, Value,
};

use super::{
    Moved, Pages, ambient_text, capital, flag, light_patch, names_under, num, patch, problem,
    settings_note, text_of, toggle, value_row, with_settings,
};
use crate::client::{Capture, Client, Snapshot};
use crate::keys::{self, KEYS};
use crate::light::{self, STEP_AMBIENT, STEP_PCT};
use crate::settings::{self as cfg, DIS_CELLS, DIS_PAGES};
use crate::slots::{self, Picked, Picker, SlotAct};
use crate::values::{NONE, show};
use crate::view::{Act, CarScreen as S, DisAct, Layout, Row, RowKind, TypeFor, View, WifiAct};

/// Longest portal PIN the entry takes.
pub(super) const PIN_MAX: usize = 8;

pub(super) fn view(pages: &Pages, snap: &Snapshot, now: Instant) -> View {
    match pages.car {
        S::Root => root(pages, snap),
        S::Setup => setup(snap),
        S::Nvme => nvme(snap, pages.moving()),
        S::Status => status(snap),
        S::AllValues => all_values(snap, now),
        S::Buses => buses(snap),
        S::Cluster => cluster(snap),
        S::ReadingPages => reading_pages(snap),
        S::ReadingPage(p) => reading_page(snap, now, p, pages.removing),
        S::Cell(p, c) => cell(snap, now, p, c),
        S::CellValue(p, c) => cell_picker(pages, snap, now, p, Some(c)),
        S::CellAdd(p) => cell_picker(pages, snap, now, p, None),
        S::CellIcon(p, c) => cell_icon(snap, p, c),
        S::Nav => nav(snap),
        S::Calls => calls(snap),
        S::Media => media(snap),
        S::Accel => accel(snap),
        S::Lighting => lighting(snap),
        S::Curve => curve(snap),
        S::Point(i) => curve_point(snap, i),
        S::Night => night(snap),
        S::Controls => controls(snap, pages.keyboard_prefs()),
        S::Wheel => key_list(snap, true),
        S::Faceplate => key_list(snap, false),
        S::Key(k) => key(snap, k),
        S::Binding(k, i) => binding(snap, k, i),
        S::Logs => logs(snap, pages),
        S::Power => power(snap),
        S::Zones => zones(snap),
        S::Zone(r) => zone(snap, r),
        S::Avsd => avsd(snap, now),
        S::Picture => picture(snap),
        S::Remote => remote(snap),
        S::Pin => pin_entry(snap, &pages.pin),
        S::Updates => updates(snap),
        S::Firmware | S::AvsdFirmware => crate::firmware::view_dev(
            &snap.firmware,
            &snap.firmware_job,
            pages.flashing(),
            pages.flashing_sector(),
        ),
        S::UpdateConfirm => update_confirm(snap),
        S::Wifi => wifi(snap),
        S::WifiNet(i) => wifi_net(snap, i, pages.forgetting()),
        S::WifiAdd => wifi_add(snap),
        S::Advanced => advanced(pages, snap),
    }
}

// ---- Helpers ----

fn set(p: SettingsPatch) -> Act {
    patch(p, false)
}

fn dis(p: DisPatch) -> Act {
    set(SettingsPatch {
        dis: Some(p),
        ..Default::default()
    })
}

fn open(screen: S) -> Act {
    Act::Car(screen)
}

fn route(label: impl Into<String>, value: impl Into<String>, to: S) -> Row {
    Row::new(RowKind::Route, label, value).act(open(to))
}

fn info(label: impl Into<String>, value: impl Into<String>) -> Row {
    Row::new(RowKind::Info, label, value)
}

/// A choice from `opts`: push and the D-pad right step on, the D-pad left steps back; both wrap.
fn choice<T: Copy + PartialEq>(
    label: &str,
    opts: &[(T, &str)],
    now: T,
    mk: impl Fn(T) -> Act,
) -> Row {
    let at = opts.iter().position(|o| o.0 == now).unwrap_or(0);
    let n = opts.len();
    let next = mk(opts[(at + 1) % n].0);
    Row::new(RowKind::Choice, label, opts[at].1)
        .act(next.clone())
        .side(mk(opts[(at + n - 1) % n].0), next)
}

/// A number stepped through `list`: the D-pad steps and stops at the ends, push steps on and
/// wraps.
fn step<T: Copy + PartialOrd>(
    label: &str,
    shown: impl Into<String>,
    list: &[T],
    now: T,
    mk: impl Fn(T) -> Act,
) -> Row {
    Row::new(RowKind::Step, label, shown)
        .act(mk(cfg::list_next(list, now)))
        .side(
            mk(cfg::list_step(list, now, -1)),
            mk(cfg::list_step(list, now, 1)),
        )
}

fn waiting(v: &mut View) {
    v.row(info("Waiting for carpid's settings", "").dimmed(true));
}

/// The cluster display's state in a few words.
fn dis_state(snap: &Snapshot) -> String {
    let shown = text_of(snap, "dis.shown");
    match (shown, text_of(snap, "dis.state")) {
        (Some(s), _) => format!("Showing {s}"),
        (None, Some("off")) => "Off".into(),
        (None, Some("waiting")) => "Waiting for the cluster".into(),
        (None, Some(s)) => capital(s),
        (None, None) => String::new(),
    }
}

// ---- Root and status ----

fn root(_pages: &Pages, snap: &Snapshot) -> View {
    let note = if snap.connected {
        settings_note(snap)
    } else {
        problem(snap)
    };
    let mut v = View::new(Layout::List, "Car Integration", true).note(note);
    let s = snap.settings.as_ref();
    if let Some(s) = s {
        v.row(
            toggle(
                "Flashing mode",
                s.flashing_mode,
                set(SettingsPatch {
                    flashing_mode: Some(!s.flashing_mode),
                    ..Default::default()
                }),
            )
            .alert(s.flashing_mode)
            .tag(if s.flashing_mode {
                "CAN sends blocked"
            } else {
                ""
            }),
        );
    }
    let status = if snap.connected {
        "Connected"
    } else {
        "Not connected"
    };
    v.row(route("Status", status, S::Status).icon("InfoOutlined"));
    let cluster = match s {
        Some(s) if s.dis.enabled => dis_state(snap),
        Some(_) => "Off".into(),
        None => String::new(),
    };
    v.row(route("Cluster display", cluster, S::Cluster).icon("SpeedOutlined"));
    let light = match (flag(snap, "light.night"), num(snap, "light.panel_pct")) {
        (Some(n), Some(p)) => format!("{}, {p:.0} %", if n { "Night" } else { "Day" }),
        (Some(n), None) => (if n { "Night" } else { "Day" }).to_string(),
        _ => String::new(),
    };
    v.row(route("Screen & lighting", light, S::Lighting).icon("Brightness6Outlined"));
    v.row(route("Controls", "", S::Controls).icon("KeyboardOutlined"));
    let debug = if snap.capture == Capture::Saving { "Collecting…" } else { "" };
    v.row(route("Debug logs", debug, S::Logs).icon("ArticleOutlined"));
    v.row(route("Power & clock", "", S::Power).icon("PowerSettingsNewOutlined"));
    let avsd = match s {
        Some(s) if !s.avsd.found && !s.avsd.ranges.is_empty() => "Not set up",
        _ if firmware_update(snap) => "Update available",
        _ => "",
    };
    v.row(route("A/V over SD", avsd, S::Avsd).icon("TvOutlined"));
    let vnc = match s {
        Some(s) if s.remote.vnc_running => "VNC on",
        _ => "",
    };
    v.row(route("Remote access", vnc, S::Remote).icon("LanOutlined"));
    v.row(route("Advanced", "", S::Advanced).icon("TuneOutlined"));
    v.finish()
}

/// The data log running now, if one is.
/// carpid offers newer bundled firmware for the RNS-E or the Teensy.
fn firmware_update(snap: &Snapshot) -> bool {
    let f = &snap.firmware;
    f.rnse.update_available() || f.teensy.update_available()
}

fn firmware_row(snap: &Snapshot, to: S) -> Row {
    let status = if firmware_update(snap) {
        "Update available"
    } else {
        ""
    };
    route("SDLink firmware", status, to)
}

// ---- Setup (docs/IMAGE-TOOL.md, NVMe and post-setup) ----

/// Rows that open the screens to visit after the first start. Back from each comes here.
fn setup(snap: &Snapshot) -> View {
    let mut v = View::new(Layout::List, "Setup", true);
    let s = snap.settings.as_ref();
    let (zone, to) = match s {
        Some(s) if s.clock.timezone.is_empty() => ("Not set".to_string(), S::Power),
        Some(s) => (
            String::new(),
            if s.clock.timezones.is_empty() {
                S::Power
            } else {
                S::Zones
            },
        ),
        None => (String::new(), S::Power),
    };
    v.row(route("Time zone", zone, to).icon("ScheduleOutlined"));
    v.row(route("Wi-Fi networks", "", S::Wifi).icon("WifiOutlined"));
    v.row(
        Row::new(RowKind::Route, "Phone connection", "")
            .icon("wirelessAa")
            .act(Act::PhoneSettings),
    );
    v.row(Row::new(
        RowKind::Info,
        "Wireless CarPlay requires supported authentication hardware.",
        "",
    ));
    v.row(route("Cluster display", "", S::Cluster).icon("SpeedOutlined"));
    v.row(route("Remote access", "", S::Remote).icon("LanOutlined"));
    if snap.storage.can_move() {
        let value = if snap.storage.job.running() {
            "Moving"
        } else {
            "Running from the SD"
        };
        v.row(route("Move to NVMe", value, S::Nvme).icon("MemoryOutlined"));
    }
    v.row(
        Row::new(RowKind::Button, "Finish setup", "")
            .button("Hide")
        .act(Act::SetupDone(true)),
    );
    v.finish()
}

/// Bytes as drive makers count them.
fn gb(bytes: u64) -> String {
    let g = bytes as f64 / 1e9;
    if g >= 100.0 {
        format!("{g:.0} GB")
    } else {
        format!("{g:.1} GB")
    }
}

fn nvme(snap: &Snapshot, asking: bool) -> View {
    let mut v = View::new(Layout::List, "Move to NVMe", true)
        .note("Copies this system onto the NVMe and starts from it. The SD card stays as a spare.");
    let st = &snap.storage;
    if let Some(n) = &st.note {
        v.row(info("Problem", n.clone()).alert(true));
    }
    let Some(s) = &st.status else {
        v.row(info("Asking carpid", "").dimmed(true));
        return v.finish();
    };
    let drive = match &s.nvme {
        Some(n) if n.model.is_empty() => gb(n.size),
        Some(n) => format!("{}, {}", n.model, gb(n.size)),
        None => "Not found".into(),
    };
    v.row(info("NVMe", drive));
    let from = match s.root_disk {
        carpi_proto::RootDisk::Sd => "SD card",
        carpi_proto::RootDisk::Nvme => "NVMe",
        carpi_proto::RootDisk::Usb => "USB drive",
        carpi_proto::RootDisk::Other => "",
    };
    v.row(info("Running from", from));
    if s.used > 0 {
        v.row(info("To copy", gb(s.used)));
    }
    let job = &st.job;
    if job.running() {
        let pct = match job.progress {
            Some((done, Some(total))) if total > 0 => format!(" {} %", done * 100 / total),
            _ => String::new(),
        };
        let msg = if job.message.is_empty() {
            "Starting".to_string()
        } else {
            capital(&job.message)
        };
        v.row(info("Moving", format!("{msg}{pct}")));
        v.row(info("Don't turn the car off", "The Pi restarts when it's done").alert(true));
        v.row(info("The phone can't be used meanwhile", "").dimmed(true));
        return v.finish();
    }
    if let Some((ok, m)) = &job.finished {
        let label = if *ok { "Done" } else { "Didn't finish" };
        v.row(info(label, capital(m)).alert(!ok));
    }
    if let Some(why) = &s.blocked {
        v.row(info("Can't start", why.clone()).alert(true));
        return v.finish();
    }
    v.row(info("Erases the NVMe", "Everything on it is lost").alert(true));
    v.row(info("Takes a few minutes", "Don't turn the car off meanwhile").dimmed(true));
    let label = if asking {
        "Confirm erase"
    } else {
        "Start the move"
    };
    let at = v.row(
        Row::new(RowKind::Button, label, "")
            .button("Start")
            .act(Act::MoveNvme)
            .alert(asking),
    );
    v.start = at;
    v.finish()
}

fn status(snap: &Snapshot) -> View {
    let mut v = View::new(Layout::List, "Status", true);
    let daemon = if snap.connected {
        snap.daemon.clone().unwrap_or_default()
    } else {
        problem(snap)
    };
    v.row(info("carpid", daemon).alert(!snap.connected));
    match snap.buses.as_deref() {
        Some(list) if !list.is_empty() => {
            for b in list {
                v.row(info(
                    format!("{} bus", b.role.to_uppercase()),
                    b.state.as_deref().unwrap_or("up").to_string(),
                ));
            }
        }
        _ => {
            v.row(info("CAN bus", "no bus data yet").dimmed(true));
        }
    }
    let on = |name: &str| match flag(snap, name) {
        Some(true) => "On",
        Some(false) => "Off",
        None => "not seen",
    };
    v.row(info("Ignition", on("ignition.kl15")));
    let rnse = match (
        flag(snap, "rnse.on_tv"),
        flag(snap, "rnse.radio_on"),
        text_of(snap, "rnse.mode"),
    ) {
        (Some(true), _, _) => "On TV".to_string(),
        (_, Some(true), Some(m)) => format!("On, {m}"),
        (_, Some(true), None) => "On".into(),
        (_, Some(false), _) => "Off".into(),
        _ => "not seen".into(),
    };
    v.row(info("RNS-E", rnse));
    let phone = match text_of(snap, "projection.kind") {
        Some(k) => {
            let kind = match k {
                "android_auto" => "Android Auto",
                "carplay" => "CarPlay",
                other => other,
            };
            match flag(snap, "projection.wireless") {
                Some(true) => format!("{kind}, wireless"),
                Some(false) => format!("{kind}, USB"),
                None => kind.to_string(),
            }
        }
        None => "None".into(),
    };
    v.row(info("Phone", phone));
    let dis = dis_state(snap);
    v.row(info(
        "Cluster display",
        if dis.is_empty() {
            "not seen".into()
        } else {
            dis
        },
    ));
    v.row(route("All values", "", S::AllValues));
    v.row(route("Buses", "", S::Buses));
    v.finish()
}

fn all_values(snap: &Snapshot, now: Instant) -> View {
    let mut v = View::new(Layout::List, "All values", true);
    v.custom = true;
    let names = names_under("", &snap.values);
    let mut any = false;
    for n in names.iter().filter(|n| snap.values.get(n).is_some()) {
        any = true;
        let mut row = value_row(n, snap, now);
        if row.label == *n {
            row.mono = true;
        }
        v.row(row);
    }
    if !any {
        v.row(info("Nothing from carpid yet", "").dimmed(true));
    }
    v.finish()
}

fn buses(snap: &Snapshot) -> View {
    let mut v = View::new(Layout::List, "Buses", true);
    v.custom = true;
    let Some(buses) = snap.buses.as_deref().filter(|b| !b.is_empty()) else {
        v.row(info("No bus data from carpid yet", "").dimmed(true));
        return v.finish();
    };
    for b in buses {
        v.section(format!("{} ({})", b.role.to_uppercase(), b.iface));
        v.row(info(
            "State",
            b.state.as_deref().unwrap_or("no NM ring seen").to_string(),
        ));
        v.row(info("Frames", b.frames.to_string()));
        for m in &b.ring {
            let mut flags = Vec::new();
            if m.sleep_ind {
                flags.push("sleep ind");
            }
            if m.sleep_ack {
                flags.push("sleep ack");
            }
            if flags.is_empty() {
                flags.push("awake");
            }
            let mut row = info(
                format!("Node {:02X}", m.node),
                format!(
                    "next {:02X} · {} · {}",
                    m.successor,
                    flags.join(", "),
                    crate::values::age(Duration::from_millis(m.age_ms))
                ),
            );
            row.mono = true;
            v.row(row);
        }
    }
    v.finish()
}

// ---- Cluster display ----

const START: [(DisPage, &str); 3] = [
    (DisPage::Readings, "Readings"),
    (DisPage::Accel, "Accel"),
    (DisPage::Media, "Media"),
];

fn cluster(snap: &Snapshot) -> View {
    with_settings(snap, "Cluster display", |v, s| {
        let d = &s.dis;
        v.row(toggle(
            "Enabled",
            d.enabled,
            set(SettingsPatch {
                dis_enabled: Some(!d.enabled),
                ..Default::default()
            }),
        ));
        if !d.enabled {
            return;
        }
        let always = d.claim == DisClaim::Always;
        dis_toggle(v, "Auto claim", always, |x| DisPatch {
            claim: Some(if x { DisClaim::Always } else { DisClaim::Tab }),
            ..Default::default()
        });
        v.row(choice("Start page", &START, d.default_page, |p| {
            dis(DisPatch {
                default_page: Some(p),
                ..Default::default()
            })
        }));
        v.row(route("Readings", "", S::ReadingPages));
        v.row(route("Accel", "", S::Accel));
        v.row(route("Navigation", "", S::Nav));
        v.row(route("Calls", "", S::Calls));
        v.row(route("Media", "", S::Media));
    })
}

/// Lines the accel test offers until the on-screen keyboard lets one be typed.
const ACCEL_PRESETS: &[&str] = &[
    "0-30mph",
    "0-60mph",
    "30-50mph",
    "40-70mph",
    "50-70mph",
    "60-100mph",
    "0-100kmh",
    "50-100kmh",
    "80-120kmh",
    "100-200kmh",
    "60ft",
    "330ft",
    "1000ft",
    "1/8mi",
    "1/4mi",
    "1/2mi",
    "1mi",
    "100m",
    "200m",
    "400m",
    "speed",
];

fn accel(snap: &Snapshot) -> View {
    with_settings(snap, "Accel", |v, s| {
        let lines = &s.dis.accel;
        for i in 0..4 {
            let now = lines.get(i).map(String::as_str).unwrap_or("");
            // Off, the presets, and what's set now when it isn't one of them.
            let mut opts: Vec<&str> = vec![""];
            if !now.is_empty() && !ACCEL_PRESETS.contains(&now) {
                opts.push(now);
            }
            opts.extend_from_slice(ACCEL_PRESETS);
            let at = opts.iter().position(|o| *o == now).unwrap_or(0);
            let n = opts.len();
            let with = |pick: &str| {
                let mut next: Vec<String> = (0..4)
                    .map(|j| {
                        if j == i {
                            pick.to_string()
                        } else {
                            lines.get(j).cloned().unwrap_or_default()
                        }
                    })
                    .collect();
                next.retain(|l| !l.is_empty());
                dis(DisPatch {
                    accel: Some(next),
                    ..Default::default()
                })
            };
            let shown = if now.is_empty() { "Off" } else { now };
            v.row(
                Row::new(RowKind::Choice, format!("Line {}", i + 1), shown)
                    .act(Act::Type(TypeFor::Accel(i)))
                    .side(with(opts[(at + n - 1) % n]), with(opts[(at + 1) % n])),
            );
        }
    })
}

/// A switch for one `[dis]` flag.
fn dis_toggle(v: &mut View, label: &str, on: bool, p: impl Fn(bool) -> DisPatch) {
    v.row(toggle(label, on, dis(p(!on))));
}

fn nav(snap: &Snapshot) -> View {
    with_settings(snap, "Navigation", |v, s| {
        let d = &s.dis;
        dis_toggle(v, "Auto claim", d.claim_nav, |x| DisPatch {
            claim_nav: Some(x),
            ..Default::default()
        });
        dis_toggle(v, "Auto release", d.release_nav, |x| DisPatch {
            release_nav: Some(x),
            ..Default::default()
        });
        const LOOKS: [(DisNavStyle, &str); 2] =
            [(DisNavStyle::Fancy, "Fancy"), (DisNavStyle::Stock, "Stock")];
        v.row(choice("Style", &LOOKS, d.nav_style, |x| {
            dis(DisPatch {
                nav_style: Some(x),
                ..Default::default()
            })
        }));
        dis_toggle(v, "Turn distance", d.nav_distance, |x| DisPatch {
            nav_distance: Some(x),
            ..Default::default()
        });
        dis_toggle(v, "Trip info", d.nav_pocket, |x| DisPatch {
            nav_pocket: Some(x),
            ..Default::default()
        });
        if d.nav_pocket {
            dis_toggle(v, "ETA", d.nav_eta, |x| DisPatch {
                nav_eta: Some(x),
                ..Default::default()
            });
            dis_toggle(v, "Time left", d.nav_time_left, |x| DisPatch {
                nav_time_left: Some(x),
                ..Default::default()
            });
            dis_toggle(v, "Distance left", d.nav_dest_distance, |x| {
                DisPatch {
                    nav_dest_distance: Some(x),
                    ..Default::default()
                }
            });
        }
        const LANES: [(DisLanes, &str); 3] = [
            (DisLanes::Off, "Off"),
            (DisLanes::Swap, "Replace road"),
            (DisLanes::Flip, "Alternate"),
        ];
        v.row(choice("Lanes", &LANES, d.nav_lanes, |x| {
            dis(DisPatch {
                nav_lanes: Some(x),
                ..Default::default()
            })
        }));
        const UNITS: [(DisUnits, &str); 3] = [
            (DisUnits::Auto, "Phone"),
            (DisUnits::Metric, "Metric"),
            (DisUnits::Imperial, "Imperial"),
        ];
        v.row(choice("Units", &UNITS, d.units, |x| {
            dis(DisPatch {
                units: Some(x),
                ..Default::default()
            })
        }));
        const CLOCKS: [(DisClock, &str); 3] = [
            (DisClock::Auto, "Phone"),
            (DisClock::H12, "12 h"),
            (DisClock::H24, "24 h"),
        ];
        v.row(choice("Clock", &CLOCKS, d.clock, |x| {
            dis(DisPatch {
                clock: Some(x),
                ..Default::default()
            })
        }));
    })
}

fn calls(snap: &Snapshot) -> View {
    with_settings(snap, "Calls", |v, s| {
        let d = &s.dis;
        dis_toggle(v, "Auto claim", d.claim_phone, |x| DisPatch {
            claim_phone: Some(x),
            ..Default::default()
        });
        dis_toggle(v, "Auto release", d.release_phone, |x| DisPatch {
            release_phone: Some(x),
            ..Default::default()
        });
    })
}

fn media(snap: &Snapshot) -> View {
    with_settings(snap, "Media", |v, s| {
        let d = &s.dis;
        dis_toggle(v, "Auto claim", d.claim_media, |x| DisPatch {
            claim_media: Some(x),
            ..Default::default()
        });
        dis_toggle(v, "Auto release", d.release_media, |x| {
            DisPatch {
                release_media: Some(x),
                ..Default::default()
            }
        });
        const LAYOUTS: [(DisMediaLayout, &str); 2] = [
            (DisMediaLayout::Stack, "Text"),
            (DisMediaLayout::Art, "Cover + text"),
        ];
        v.row(choice("Layout", &LAYOUTS, d.media_layout, |x| {
            dis(DisPatch {
                media_layout: Some(x),
                ..Default::default()
            })
        }));
        let shown = if d.cover_flash_s <= 0.0 {
            "Off".to_string()
        } else {
            format!("{} s", d.cover_flash_s)
        };
        v.row(step(
            "Fullscreen cover time",
            shown,
            &cfg::COVER_FLASH,
            d.cover_flash_s,
            |x| {
                dis(DisPatch {
                    cover_flash_s: Some(x),
                    ..Default::default()
                })
            },
        ));
    })
}

// ---- Readings pages ----

fn dis_act(a: DisAct) -> Act {
    Act::Dis(a)
}

fn reading_pages(snap: &Snapshot) -> View {
    with_settings(snap, "Readings", |v, s| {
        let pages = cfg::dis_pages(&s.dis);
        let n = pages.len();
        for (i, p) in pages.iter().enumerate() {
            let left = dis_act(DisAct::MovePage(i, i.saturating_sub(1)));
            let right = dis_act(DisAct::MovePage(i, (i + 1).min(n - 1)));
            v.row(route(p.name.clone(), "", S::ReadingPage(i)).side(left, right));
        }
        if n < DIS_PAGES {
            v.row(
                Row::new(RowKind::Button, "Add page", "")
                    .button("Add")
                    .act(dis_act(DisAct::AddPage)),
            );
        }
    })
}

/// A cell's live reading, as the cluster shows it.
fn cell_reading(c: &carpi_proto::DisCellSetting, snap: &Snapshot, now: Instant) -> String {
    let seen = snap.values.get(&c.key);
    let s = show(&c.key, seen, now);
    let Some(Value::Num(x)) = seen.and_then(|v| v.value.as_ref()) else {
        return match seen {
            None => "not seen".into(),
            Some(_) => slots::reading(&s.value, &s.unit),
        };
    };
    let unit = c.unit.clone().unwrap_or(s.unit.clone());
    match cfg::convert(x * c.scale, &s.unit, &unit) {
        Some(y) => slots::reading(&format!("{:.*}", usize::from(c.decimals), y), &unit),
        None => slots::reading(&s.value, &s.unit),
    }
}

fn page_of(s: &Settings, p: usize) -> Option<DisPageSetting> {
    cfg::dis_pages(&s.dis).get(p).cloned()
}

fn reading_page(snap: &Snapshot, now: Instant, p: usize, removing: bool) -> View {
    let title = snap
        .settings
        .as_ref()
        .and_then(|s| page_of(s, p))
        .map_or_else(|| "Readings page".to_string(), |pg| pg.name);
    with_settings(snap, &title, |v, s| {
        let pages = cfg::dis_pages(&s.dis);
        let Some(page) = pages.get(p) else {
            v.row(info("This page is gone", "").dimmed(true));
            return;
        };
        v.row(
            Row::new(RowKind::Choice, "Name", page.name.clone())
                .act(Act::Type(TypeFor::PageName(p)))
                .side(dis_act(DisAct::Name(p, -1)), dis_act(DisAct::Name(p, 1))),
        );
        for (c, cell) in page.cells.iter().enumerate() {
            // The DIS shows cells 1-4 down the left, 5-8 down the right.
            if c == 0 || c == DIS_CELLS / 2 {
                let col = if c == 0 {
                    "Left column"
                } else {
                    "Right column"
                };
                v.row(info(col, "").dimmed(true));
            }
            v.row(
                route(
                    slots::label(&cell.key),
                    cell_reading(cell, snap, now),
                    S::Cell(p, c),
                )
                .side(
                    dis_act(DisAct::MoveCell(p, c, -1)),
                    dis_act(DisAct::MoveCell(p, c, 1)),
                ),
            );
        }
        if page.cells.len() < DIS_CELLS {
            v.row(route("Add value", "", S::CellAdd(p)));
        }
        let n = pages.len();
        if n > 1 {
            v.row(
                Row::new(RowKind::Step, "Position", format!("{} of {n}", p + 1))
                    .act(dis_act(DisAct::MovePage(p, (p + 1) % n)))
                    .side(
                        dis_act(DisAct::MovePage(p, p.saturating_sub(1))),
                        dis_act(DisAct::MovePage(p, (p + 1).min(n - 1))),
                    ),
            );
            let label = if removing {
                "Confirm remove"
            } else {
                "Remove page"
            };
            v.row(
                Row::new(RowKind::Button, label, "")
                    .button("Remove")
                    .act(dis_act(DisAct::RemovePage(p)))
                    .alert(removing),
            );
        }
    })
}

fn cell(snap: &Snapshot, now: Instant, p: usize, c: usize) -> View {
    let cell = snap
        .settings
        .as_ref()
        .and_then(|s| page_of(s, p))
        .and_then(|pg| pg.cells.get(c).cloned());
    let title = cell
        .as_ref()
        .map_or_else(|| "Value".to_string(), |x| slots::label(&x.key));
    with_settings(snap, &title, |v, _| {
        let Some(cell) = cell else {
            v.row(info("This value is gone", "").dimmed(true));
            return;
        };
        v.row(route(
            "Value",
            cell_reading(&cell, snap, now),
            S::CellValue(p, c),
        ));
        const DECIMALS: [u8; 4] = [0, 1, 2, 3];
        v.row(step(
            "Decimals",
            cell.decimals.to_string(),
            &DECIMALS,
            cell.decimals,
            |d| dis_act(DisAct::Decimals(p, c, d)),
        ));
        let own = crate::defs::find(&cell.key).map_or("", |d| d.unit);
        let units = cfg::units_for(own);
        if units.len() > 1 {
            let now_unit = cell.unit.clone().unwrap_or_else(|| own.to_string());
            let at = units.iter().position(|u| *u == now_unit).unwrap_or(0);
            let n = units.len();
            let mk = |i: usize| {
                let u = units[i % n];
                dis_act(DisAct::Unit(p, c, (u != own).then(|| u.to_string())))
            };
            v.row(
                Row::new(RowKind::Choice, "Unit", units[at])
                    .act(mk(at + 1))
                    .side(mk(at + n - 1), mk(at + 1)),
            );
        }
        v.row(route("Icon", "", S::CellIcon(p, c)));
        v.row(
            Row::new(RowKind::Button, "Remove value", "")
                .button("Remove")
                .act(dis_act(DisAct::RemoveCell(p, c))),
        );
    })
}

/// The picker for a cell's value; `c` `None` adds one.
fn cell_picker(pages: &Pages, snap: &Snapshot, now: Instant, p: usize, c: Option<usize>) -> View {
    let current = snap
        .settings
        .as_ref()
        .and_then(|s| page_of(s, p))
        .and_then(|pg| c.and_then(|c| pg.cells.get(c).map(|x| x.key.clone())));
    let title = if c.is_some() { "Value" } else { "Add value" };
    let mut v = pages.reading.view(
        title,
        current.as_deref(),
        pages.recent(),
        snap,
        now,
        &slots::DIS,
    );
    v.note = settings_note(snap);
    v
}

fn cell_icon(snap: &Snapshot, p: usize, c: usize) -> View {
    let mut v = with_settings(snap, "Icon", |v, s| {
        let Some(cell) = page_of(s, p).and_then(|pg| pg.cells.get(c).cloned()) else {
            return;
        };
        let now = cell.icon.as_deref();
        let at = v.row(
            info("Auto", "")
                .act(dis_act(DisAct::Icon(p, c, None)))
                .checked(now.is_none()),
        );
        v.start = at;
        for name in cfg::CELL_ICONS {
            let at = v.row(
                info(capital(name), "")
                    .act(dis_act(DisAct::Icon(p, c, Some(name.to_string()))))
                    .checked(now == Some(name)),
            );
            if now == Some(name) {
                v.start = at;
            }
        }
    });
    v.select = true;
    v
}

// ---- Screen & lighting ----

fn lighting(snap: &Snapshot) -> View {
    let pct = num(snap, "light.panel_pct");
    let panel = pct.map_or_else(|| NONE.to_string(), |p| format!("{p:.0} %"));
    with_settings(snap, "Screen & lighting", |v, s| {
        let l = &s.light;
        v.row(toggle(
            "Auto brightness",
            l.auto_brightness,
            light_patch(
                LightPatch {
                    auto_brightness: Some(!l.auto_brightness),
                    ..Default::default()
                },
                false,
            ),
        ));
        let nudge = |delta_pct| Act::Light(LightAction::Nudge { delta_pct });
        if snap.connected && l.auto_brightness && pct.is_some() {
            v.row(
                Row::new(RowKind::Step, "Brightness now", panel.clone())
                    .act(nudge(STEP_PCT as i8))
                    .side(nudge(-(STEP_PCT as i8)), nudge(STEP_PCT as i8)),
            );
        } else if !l.auto_brightness {
            v.row(info("Brightness now", panel));
        }
        if !l.auto_brightness {
            v.row(route("Night mode", night_now(snap), S::Night));
            return;
        }
        let source = |x| {
            light_patch(
                LightPatch {
                    ambient_source: Some(x),
                    ..Default::default()
                },
                false,
            )
        };
        v.row(
            Row::new(
                RowKind::Choice,
                "Light sensor",
                capital(light::source_name(l.ambient_source)),
            )
            .act(source(light::next_source(l.ambient_source)))
            .side(
                source(light::prev_source(l.ambient_source)),
                source(light::next_source(l.ambient_source)),
            ),
        );
        use carpi_proto::CurveMode;
        const MODES: [(CurveMode, &str); 2] = [
            (CurveMode::Learned, "Learned"),
            (CurveMode::Manual, "Manual"),
        ];
        v.row(choice("Curve", &MODES, l.curve_mode, |m| {
            light_patch(
                LightPatch {
                    curve_mode: Some(m),
                    ..Default::default()
                },
                false,
            )
        }));
        let learned = l
            .learned
            .iter()
            .filter(|p| p.source == l.ambient_source)
            .count();
        if snap.connected && learned > 0 {
            v.row(
                Row::new(RowKind::Button, "Learned points", learned.to_string())
                    .button("Reset")
                    .act(Act::Light(LightAction::ResetLearned { source: None })),
            );
        }
        v.row(toggle(
            "Scroll trim",
            l.scroll_trim,
            light_patch(
                LightPatch {
                    scroll_trim: Some(!l.scroll_trim),
                    ..Default::default()
                },
                false,
            ),
        ));
        v.row(route("Brightness curve", "", S::Curve));
        v.row(route("Night mode", night_now(snap), S::Night));
    })
}

fn night_now(snap: &Snapshot) -> String {
    let night = match flag(snap, "light.night") {
        Some(true) => "Night",
        Some(false) => "Day",
        None => return String::new(),
    };
    match text_of(snap, "light.night_source") {
        Some(by) => format!("{night}, by the {by}"),
        None => night.to_string(),
    }
}

/// The plot, with the manual curve's points to edit.
fn curve(snap: &Snapshot) -> View {
    let settings = snap.settings.as_ref().map(|s| &s.light);
    let ambient = num(snap, "light.ambient");
    let pct = num(snap, "light.panel_pct");
    let c = text_of(snap, super::CURVE).and_then(light::parse);
    let mut v = View::new(Layout::List, "Brightness curve", true).note(settings_note(snap));
    v.custom = true;
    v.plot = Some(light::plot(c.as_ref(), settings, ambient, pct, true));
    v.row(info(
        "Ambient",
        ambient.map_or_else(|| NONE.to_string(), ambient_text),
    ));
    let Some(l) = settings else {
        waiting(&mut v);
        return v.finish();
    };
    for (i, p) in l.manual_curve.iter().enumerate() {
        let at = v.row(route(
            format!("Point {}", i + 1),
            format!("sensor {:.0} → {} %", light::raw(p.ambient), p.pct),
            S::Point(i),
        ));
        if i == 0 {
            v.start = at;
        }
    }
    if let Some(here) = ambient {
        let level = pct.map_or(50, |p| p.round().clamp(0.0, 100.0) as u8);
        v.row(
            Row::new(RowKind::Button, "Add point", format!("{level} %"))
                .button("Add")
                .act(light_patch(
                    LightPatch {
                        manual_curve: Some(light::added(&l.manual_curve, here, level)),
                        ..Default::default()
                    },
                    false,
                )),
        );
    }
    v.finish()
}

fn curve_point(snap: &Snapshot, at: usize) -> View {
    with_settings(snap, &format!("Point {}", at + 1), |v, s| {
        let c = &s.light.manual_curve;
        let Some(p) = c.get(at) else {
            v.row(info("This point is gone", "").dimmed(true));
            return;
        };
        let edit = |pct: i16, ambient: f64| {
            light_patch(
                LightPatch {
                    manual_curve: Some(light::moved(c, at, pct, ambient)),
                    ..Default::default()
                },
                false,
            )
        };
        v.row(
            Row::new(RowKind::Step, "Panel", format!("{} %", p.pct))
                .act(edit(STEP_PCT, 0.0))
                .side(edit(-STEP_PCT, 0.0), edit(STEP_PCT, 0.0)),
        );
        v.row(
            Row::new(RowKind::Step, "Ambient", ambient_text(p.ambient))
                .act(edit(0, STEP_AMBIENT))
                .side(edit(0, -STEP_AMBIENT), edit(0, STEP_AMBIENT)),
        );
        if c.len() > 2 {
            v.row(
                Row::new(RowKind::Button, "Remove point", "")
                    .button("Remove")
                    .act(light_patch(
                        LightPatch {
                            manual_curve: Some(light::removed(c, at)),
                            ..Default::default()
                        },
                        true,
                    )),
            );
        }
    })
}

/// `HH:MM` moved by `mins`, round the clock.
fn clock_step(t: &str, mins: i32) -> String {
    let (h, m) = t.split_once(':').unwrap_or(("0", "0"));
    let at = h.trim().parse::<i32>().unwrap_or(0) * 60 + m.trim().parse::<i32>().unwrap_or(0);
    let to = (at + mins).rem_euclid(24 * 60);
    format!("{:02}:{:02}", to / 60, to % 60)
}

fn night(snap: &Snapshot) -> View {
    with_settings(snap, "Night mode", |v, s| {
        let l = &s.light;
        let now = night_now(snap);
        v.row(info("Now", if now.is_empty() { NONE.into() } else { now }));
        const BY: [(DarkModeSource, &str); 2] = [
            (DarkModeSource::Headlights, "Headlights"),
            (DarkModeSource::Ambient, "Light sensor"),
        ];
        v.row(choice("Source", &BY, l.dark_mode_source, |x| {
            light_patch(
                LightPatch {
                    dark_mode_source: Some(x),
                    ..Default::default()
                },
                false,
            )
        }));
        if l.dark_mode_source == DarkModeSource::Ambient {
            let th = |d: f64| {
                light_patch(
                    LightPatch {
                        night_threshold: Some(((l.night_threshold + d) * 100.0).round() / 100.0),
                        ..Default::default()
                    },
                    false,
                )
            };
            v.row(
                Row::new(
                    RowKind::Step,
                    "Night below",
                    ambient_text(l.night_threshold),
                )
                .act(th(STEP_AMBIENT))
                .side(th(-STEP_AMBIENT), th(STEP_AMBIENT)),
            );
        }
        match (l.latitude, l.longitude) {
            (Some(la), Some(lo)) => {
                v.row(info("Location", format!("{la:.1}, {lo:.1}")));
                v.row(
                    Row::new(RowKind::Button, "Forget location", "")
                        .button("Forget")
                        .act(light_patch(
                            LightPatch {
                                clear_location: true,
                                ..Default::default()
                            },
                            false,
                        )),
                );
            }
            _ => {
                let time = |label: &str, now: &str, mk: fn(String) -> LightPatch| {
                    let at = |d| light_patch(mk(clock_step(now, d)), false);
                    Row::new(RowKind::Step, label, now.to_string())
                        .act(at(30))
                        .side(at(-30), at(30))
                };
                v.row(time("Night from", &l.night_from, |t| LightPatch {
                    night_from: Some(t),
                    ..Default::default()
                }));
                v.row(time("Day from", &l.day_from, |t| LightPatch {
                    day_from: Some(t),
                    ..Default::default()
                }));
                v.row(info("Location", "Not set").dimmed(true));
            }
        }
    })
}

// ---- Controls ----

fn input(p: InputPatch) -> Act {
    set(SettingsPatch {
        input: Some(p),
        ..Default::default()
    })
}

fn controls(snap: &Snapshot, prefs: crate::keyboard::Prefs) -> View {
    with_settings(snap, "Controls", |v, s| {
        v.row(route("Steering wheel", "", S::Wheel));
        v.row(route("Faceplate", "", S::Faceplate));
        const LONG: [u32; 13] = [
            200, 250, 300, 350, 400, 440, 500, 600, 700, 800, 1000, 1200, 1500,
        ];
        v.row(step(
            "Long press",
            format!("{} ms", s.input.long_ms),
            &LONG,
            s.input.long_ms,
            |x| {
                input(InputPatch {
                    long_ms: Some(x),
                    ..Default::default()
                })
            },
        ));
        const HELD: [u32; 9] = [3, 4, 5, 6, 8, 10, 12, 15, 20];
        v.row(step(
            "Extended hold",
            format!("{} s", s.input.extended_s),
            &HELD,
            s.input.extended_s,
            |x| {
                input(InputPatch {
                    extended_s: Some(x),
                    ..Default::default()
                })
            },
        ));
        keyboard_rows(v, prefs);
        if s.bindings_custom {
            v.row(
                Row::new(RowKind::Button, "Key bindings", "Changed")
                    .button("Reset to defaults")
                    .act(set(SettingsPatch {
                        bindings_reset: true,
                        ..Default::default()
                    })),
            );
        } else {
            v.row(info("Key bindings", "Defaults").dimmed(true));
        }
    })
}

/// The on-screen keyboard's settings. They live in the pages' own file, not carpid's.
fn keyboard_rows(v: &mut View, p: crate::keyboard::Prefs) {
    use crate::keyboard::{Order, Style};
    const STYLES: [(Style, &str); 4] = [
        (Style::Auto, "Auto"),
        (Style::Knob, "Knob"),
        (Style::Touch, "Touch"),
        (Style::Off, "Off"),
    ];
    v.row(choice("On-screen keyboard", &STYLES, p.style, |x| {
        Act::Keyboard(crate::keyboard::Prefs { style: x, ..p })
    }));
    if matches!(p.style, Style::Auto | Style::Knob) {
        const ORDERS: [(Order, &str); 2] = [
            (Order::Alphabetical, "Alphabetical"),
            (Order::Qwerty, "QWERTY"),
        ];
        v.row(choice("Key order", &ORDERS, p.order, |x| {
            Act::Keyboard(crate::keyboard::Prefs { order: x, ..p })
        }));
    }
    v.row(toggle(
        "Show passwords",
        p.show_secret,
        Act::Keyboard(crate::keyboard::Prefs {
            show_secret: !p.show_secret,
            ..p
        }),
    ));
}

fn key_list(snap: &Snapshot, wheel: bool) -> View {
    let title = if wheel { "Steering wheel" } else { "Faceplate" };
    with_settings(snap, title, |v, s| {
        let list = keys::bound(&s.bindings, wheel);
        if list.is_empty() {
            v.row(info("No keys bound", "").dimmed(true));
        }
        for k in list {
            v.row(route(KEYS[k].label, "", S::Key(k)));
        }
    })
}

fn key(snap: &Snapshot, k: usize) -> View {
    let Some(name) = KEYS.get(k) else {
        return View::new(Layout::List, "Key", true).finish();
    };
    with_settings(snap, name.label, |v, s| {
        let rows: Vec<(usize, &carpi_proto::Binding)> = s
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.key == name.key)
            .collect();
        let tables = {
            let mut t: Vec<&str> = rows.iter().map(|(_, b)| b.table.as_str()).collect();
            t.dedup();
            t.len()
        };
        for (i, b) in rows {
            let mut label = keys::gesture_label(&b.gesture, s.input.extended_s);
            if tables > 1 {
                label = format!("{label}, {}", keys::table_label(&b.table));
            }
            // Push opens the list; the D-pad steps through it in place.
            let list = cfg::actions(&b.table);
            let at = list.iter().position(|a| *a == b.action).unwrap_or(0);
            let n = list.len().max(1);
            let to = |j: usize| {
                let a = list.get(j % n).copied().unwrap_or(b.action.as_str());
                set(SettingsPatch {
                    bindings: Some(cfg::rebound(&s.bindings, i, a)),
                    ..Default::default()
                })
            };
            v.row(
                Row::new(RowKind::Choice, label, keys::action_label(&b.action))
                    .act(open(S::Binding(k, i)))
                    .side(to(at + n - 1), to(at + 1)),
            );
        }
    })
}

fn binding(snap: &Snapshot, _k: usize, at: usize) -> View {
    let title = snap
        .settings
        .as_ref()
        .and_then(|s| s.bindings.get(at))
        .map_or_else(
            || "Key".to_string(),
            |b| {
                let held = snap.settings.as_ref().map_or(10, |s| s.input.extended_s);
                format!(
                    "{} {}",
                    keys::key_label(&b.key),
                    keys::gesture_label(&b.gesture, held).to_lowercase()
                )
            },
        );
    let mut v = with_settings(snap, &title, |v, s| {
        let Some(b) = s.bindings.get(at) else {
            v.row(info("This key is gone", "").dimmed(true));
            return;
        };
        let mut actions: Vec<&str> = cfg::actions(&b.table).to_vec();
        if !actions.contains(&b.action.as_str()) {
            actions.insert(0, &b.action);
        }
        for a in actions {
            let p = SettingsPatch {
                bindings: Some(cfg::rebound(&s.bindings, at, a)),
                ..Default::default()
            };
            let row = v.row(
                info(keys::action_label(a), "")
                    .act(patch(p, true))
                    .checked(a == b.action),
            );
            if a == b.action {
                v.start = row;
            }
        }
    });
    v.select = true;
    v
}

// ---- Debug logs ----

fn logs_patch(p: LogsPatch) -> Act {
    set(SettingsPatch {
        logs: Some(p),
        ..Default::default()
    })
}

fn logs(snap: &Snapshot, pages: &Pages) -> View {
    let _ = pages;
    with_settings(snap, "Debug logs", |v, s| {
        if snap.capture == Capture::Saving {
            v.row(info("Collect debug logs", "Collecting…").dimmed(true));
        } else if snap.connected {
            v.row(
                Row::new(RowKind::Button, "Collect debug logs", "")
                    .button("Collect")
                    .act(Act::Capture),
            );
        }
        match &snap.capture {
            Capture::Saved { label, .. } => {
                v.row(info("Last", label.clone()));
            }
            Capture::Failed(why) => {
                v.row(info("Last failed", why.clone()).alert(true));
            }
            _ => {}
        }
        const MINUTES: [u32; 8] = [1, 2, 3, 5, 10, 15, 20, 30];
        v.row(step(
            "Length",
            format!("{} min", s.logs.capture_minutes),
            &MINUTES,
            s.logs.capture_minutes,
            |x| {
                logs_patch(LogsPatch {
                    capture_minutes: Some(x),
                    ..Default::default()
                })
            },
        ));
        const PCT: [u8; 15] = [5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75];
        v.row(step(
            "Log storage",
            format!("{} %", s.logs.max_pct),
            &PCT,
            s.logs.max_pct,
            |x| {
                logs_patch(LogsPatch {
                    max_pct: Some(x),
                    ..Default::default()
                })
            },
        ));
    })
}

// ---- Power & clock ----

fn power_patch(p: PowerPatch) -> Act {
    set(SettingsPatch {
        power: Some(p),
        ..Default::default()
    })
}

fn clock_patch(p: ClockPatch) -> Act {
    set(SettingsPatch {
        clock: Some(p),
        ..Default::default()
    })
}

fn power(snap: &Snapshot) -> View {
    with_settings(snap, "Power & clock", |v, s| {
        let p = &s.power;
        v.row(toggle(
            "Auto shutdown",
            p.shutdown,
            power_patch(PowerPatch {
                shutdown: Some(!p.shutdown),
                ..Default::default()
            }),
        ));
        if p.shutdown {
            const SILENT: [u32; 9] = [15, 30, 45, 60, 90, 120, 180, 300, 600];
            v.row(step(
                "Shutdown delay",
                format!("{} s", p.silent_s),
                &SILENT,
                p.silent_s,
                |x| {
                    power_patch(PowerPatch {
                        silent_s: Some(x),
                        ..Default::default()
                    })
                },
            ));
            const HOLD: [u32; 10] = [0, 5, 10, 15, 20, 30, 45, 60, 90, 120];
            let shown = if p.radio_hold_min == 0 {
                "Off".to_string()
            } else {
                format!("{} min", p.radio_hold_min)
            };
            v.row(step(
                "Radio hold",
                shown,
                &HOLD,
                p.radio_hold_min,
                |x| {
                    power_patch(PowerPatch {
                        radio_hold_min: Some(x),
                        ..Default::default()
                    })
                },
            ));
        }
        let c = &s.clock;
        v.row(toggle(
            "Clock from car",
            c.set_from_car,
            clock_patch(ClockPatch {
                set_from_car: Some(!c.set_from_car),
                ..Default::default()
            }),
        ));
        let zone = if c.timezone.is_empty() {
            "Unknown".to_string()
        } else {
            c.timezone.replace('_', " ")
        };
        if c.timezones.is_empty() {
            v.row(info("Time zone", zone));
        } else {
            v.row(route("Time zone", "", S::Zones));
        }
    })
}

fn zones(snap: &Snapshot) -> View {
    with_settings(snap, "Time zone", |v, s| {
        let now = s.clock.timezone.split('/').next().unwrap_or("");
        for (i, (region, _)) in cfg::zone_regions(&s.clock.timezones).iter().enumerate() {
            let at = v.row(route(region.clone(), "", S::Zone(i)));
            if region == now {
                v.start = at;
            }
        }
    })
}

fn zone(snap: &Snapshot, r: usize) -> View {
    let regions = snap
        .settings
        .as_ref()
        .map(|s| cfg::zone_regions(&s.clock.timezones))
        .unwrap_or_default();
    let title = regions.get(r).map_or("Time zone", |x| x.0.as_str());
    let mut v = with_settings(snap, title, |v, s| {
        let Some((_, list)) = regions.get(r) else {
            return;
        };
        for z in list {
            let name = z.split_once('/').map_or(z.as_str(), |x| x.1);
            let at = v.row(
                info(name.replace('_', " "), "")
                    .act(Act::Zone(z.clone()))
                    .checked(*z == s.clock.timezone),
            );
            if *z == s.clock.timezone {
                v.start = at;
            }
        }
    });
    v.select = true;
    v
}

// ---- A/V over SD ----

fn avsd_patch(key: &str, value: f64) -> Act {
    let mut p = AvsdPatch::default();
    p.values.insert(key.to_string(), value);
    set(SettingsPatch {
        avsd: Some(p),
        ..Default::default()
    })
}

/// A number as the range steps it, with its unit; 0 as Off where that means off.
fn avsd_text(r: &AvsdRange, x: f64) -> String {
    if r.zero_off && x == 0.0 {
        return "Off".into();
    }
    let dp = if r.step >= 1.0 || r.step <= 0.0 {
        0
    } else {
        (-r.step.log10()).ceil() as usize
    };
    let n = format!("{x:.dp$}");
    if r.unit.is_empty() {
        n
    } else {
        format!("{n} {}", r.unit)
    }
}

/// One avsd.env setting as a row: a switch for 0/1, a stepped number otherwise.
fn avsd_row(r: &AvsdRange, now: f64) -> Row {
    if r.min == 0.0 && r.max == 1.0 && r.step == 1.0 {
        return toggle(
            &r.label,
            now != 0.0,
            avsd_patch(&r.key, if now != 0.0 { 0.0 } else { 1.0 }),
        );
    }
    let step = if r.step > 0.0 { r.step } else { 1.0 };
    let snap = |x: f64| ((x - r.min) / step).round() * step + r.min;
    let lo = if r.zero_off { 0.0 } else { r.min };
    let down = if r.zero_off && now <= r.min {
        0.0
    } else {
        snap(now - step).max(r.min)
    };
    let up = if r.zero_off && now == 0.0 {
        r.min
    } else {
        snap(now + step).min(r.max)
    };
    let next = if now >= r.max { lo } else { up };
    Row::new(RowKind::Step, r.label.clone(), avsd_text(r, now))
        .act(avsd_patch(&r.key, next))
        .side(avsd_patch(&r.key, down), avsd_patch(&r.key, up))
}

fn avsd_value(s: &Settings, r: &AvsdRange) -> f64 {
    s.avsd.values.get(&r.key).copied().unwrap_or(r.default)
}

const GAIN: &str = "GAIN";

fn avsd(snap: &Snapshot, now: Instant) -> View {
    with_settings(snap, "A/V over SD", |v, s| {
        v.row(info("RNS-E firmware", snap.firmware.rnse.installed.label()));
        v.row(info("SDLink protocol", snap.firmware.protocol.label()));
        v.row(firmware_row(snap, S::AvsdFirmware));
        let decoder = &snap.decoder;
        let reading = decoder.running();
        let result = match &decoder.finished {
            Some((true, crc)) => format!("CRC32 {crc}"),
            Some((false, error)) => error.clone(),
            None if reading => match decoder.progress {
                Some((done, Some(total))) if total > 0 => {
                    format!("Reading… {}%", done * 100 / total)
                }
                _ => "Reading… this can take several minutes".into(),
            },
            None => "Sector 107 CRC32".into(),
        };
        v.row(
            Row::new(RowKind::Button, "Legacy decoder CRC", result)
                .tag("Diagnostic only")
                .button(if reading { "Reading…" } else { "Read CRC" })
                .act(if reading { Act::None } else { Act::DecoderCrc })
                .dimmed(reading)
                .alert(decoder.finished.as_ref().is_some_and(|(ok, _)| !ok)),
        );
        if s.avsd.ranges.is_empty() {
            v.row(info("Waiting for carpid's A/V settings", "").dimmed(true));
            return;
        }
        if !s.avsd.found {
            v.row(info("Not set up", s.avsd.path.clone()).alert(true));
        }
        let status: Vec<String> = names_under("avsd.", &snap.values)
            .into_iter()
            .filter(|n| snap.values.get(n).is_some())
            .collect();
        if status.is_empty() {
            v.row(info("Stream", "Not running").dimmed(true));
        }
        for n in status {
            v.row(value_row(&n, snap, now));
        }
        if let Some(r) = s.avsd.ranges.iter().find(|r| r.key == GAIN) {
            v.row(avsd_row(r, avsd_value(s, r)).tap(|row| row.label = "Audio gain".into()));
        }
        let n = s.avsd.ranges.iter().filter(|r| r.key != GAIN).count();
        if n > 0 {
            v.row(route("Picture", "", S::Picture));
        }
        v.row(
            Row::new(RowKind::Button, "Restart stream", "")
                .button("Restart")
                .act(set(SettingsPatch {
                    avsd: Some(AvsdPatch {
                        restart: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
        );
    })
}

fn picture(snap: &Snapshot) -> View {
    with_settings(snap, "Picture", |v, s| {
        for r in s.avsd.ranges.iter().filter(|r| r.key != GAIN) {
            v.row(avsd_row(r, avsd_value(s, r)));
        }
    })
}

// ---- Remote access ----

fn remote_patch(p: carpi_proto::RemotePatch) -> Act {
    set(SettingsPatch {
        remote: Some(p),
        ..Default::default()
    })
}

fn remote(snap: &Snapshot) -> View {
    with_settings(snap, "Remote access", |v, s| {
        v.row(info(
            "Portal",
            s.portal.address.clone().unwrap_or_else(|| "Off".into()),
        ));
        v.row(route("PIN", "", S::Pin));
        if s.portal_pin_set {
            v.row(
                Row::new(RowKind::Button, "Clear PIN", "")
                    .button("Clear")
                    .act(set(SettingsPatch {
                        portal_pin: Some(PinChange::Clear),
                        ..Default::default()
                    })),
            );
            v.row(toggle(
                "PIN for downloads",
                s.portal.protect_downloads,
                set(SettingsPatch {
                    portal: Some(PortalPatch {
                        protect_downloads: Some(!s.portal.protect_downloads),
                    }),
                    ..Default::default()
                }),
            ));
        }
        v.row(info("SSH", "Always on"));
        let r = &s.remote;
        if !r.vnc_installed {
            v.row(info("VNC", "Not installed").dimmed(true));
            return;
        }
        v.row(toggle(
            "VNC",
            r.vnc_running,
            remote_patch(carpi_proto::RemotePatch {
                vnc_running: Some(!r.vnc_running),
                ..Default::default()
            }),
        ));
        v.row(toggle(
            "VNC at boot",
            r.vnc_at_boot,
            remote_patch(carpi_proto::RemotePatch {
                vnc_at_boot: Some(!r.vnc_at_boot),
                ..Default::default()
            }),
        ));
    })
}

fn pin_entry(snap: &Snapshot, pin: &str) -> View {
    let mut v = View::new(Layout::List, "New PIN", true).note(settings_note(snap));
    let shown = if pin.is_empty() {
        String::new()
    } else {
        "•".repeat(pin.len())
    };
    v.row(info("New PIN", shown).dimmed(pin.is_empty()));
    if !pin.is_empty() && snap.connected {
        v.row(
            Row::new(RowKind::Button, "Save PIN", "")
                .button("Save")
                .act(Act::PinSave),
        );
    }
    if !pin.is_empty() {
        v.row(
            Row::new(RowKind::Button, "Delete digit", "")
                .button("Delete")
                .act(Act::PinDelete),
        );
    }
    if pin.len() < PIN_MAX {
        let mut first = -1;
        for d in (1..=9).chain([0]) {
            let at = v.row(
                Row::new(RowKind::Button, d.to_string(), "")
                    .button("Add")
                    .act(Act::PinDigit(d)),
            );
            if first < 0 {
                first = at;
            }
        }
        v.start = first;
    }
    v.finish()
}

// ---- Updates and advanced ----

fn updates(snap: &Snapshot) -> View {
    with_settings(snap, "Updates", |v, s| {
        // "carpid 0.1.0, release 2026.11.08 (stable)": the release is what updates change.
        let version = match s.update.version.split_once("release ") {
            Some((_, r)) => r.to_string(),
            None if s.update.version.is_empty() => snap.daemon.clone().unwrap_or_default(),
            None => s.update.version.clone(),
        };
        let version = snap
            .update_status
            .as_ref()
            .and_then(|s| s.current.clone())
            .unwrap_or(version);
        v.row(info("Version", version));
        // One row for how the last update went: this session's attempt if there was one, else
        // what carpi-update recorded.
        let job = &snap.update;
        let last = if let Some((ok, m)) = &job.finished {
            Some((
                if *ok {
                    capital(m)
                } else {
                    format!("Failed: {m}")
                },
                !ok,
            ))
        } else if let Some(n) = &snap.update_note {
            Some((n.clone(), false))
        } else if let Some(status) = &snap.update_status
            && let Some(last) = status.last_apply.as_deref()
        {
            let history = match (last, status.update_running) {
                ("ok", _) => "Succeeded",
                ("rolled_back", _) => "Rolled back",
                ("failed", _) => "Failed",
                ("pending_boot", _) => "First install prepared",
                ("applying", Some(true)) => "In progress",
                ("applying", Some(false)) => "Interrupted",
                _ => "Unknown",
            };
            let alert = matches!(
                (last, status.update_running),
                ("failed" | "rolled_back", _) | ("applying", Some(false))
            );
            Some((history.to_string(), alert))
        } else {
            None
        };
        if let Some((value, alert)) = last {
            v.row(info("Last update", value).alert(alert));
        }
        const CHANNELS: [(UpdateChannel, &str); 2] = [
            (UpdateChannel::Stable, "Stable"),
            (UpdateChannel::Beta, "Beta"),
        ];
        v.row(choice("Channel", &CHANNELS, s.update.channel, |c| {
            set(SettingsPatch {
                update: Some(UpdatePatch { channel: Some(c) }),
                ..Default::default()
            })
        }));
        if job.running() {
            let pct = match job.progress {
                Some((done, Some(total))) if total > 0 => {
                    format!(" {} %", done * 100 / total)
                }
                _ => String::new(),
            };
            let stage = if job.message.is_empty() {
                capital(&job.stage)
            } else {
                job.message.clone()
            };
            v.row(info("Updating", format!("{stage}{pct}")));
        } else if snap.update_recovering {
            v.row(info("Status", "Checking"));
        } else {
            v.row(route("Install", "", S::UpdateConfirm));
        }
        v.row(route("Wi-Fi networks", "", S::Wifi).icon("WifiOutlined"));
        v.row(firmware_row(snap, S::Firmware));
    })
}

// ---- Wi-Fi networks (docs/PAGES.md 5b) ----

/// The last scan or test, as a row: running, or how it ended.
fn wifi_job_row(v: &mut View, snap: &Snapshot, what: &str) {
    let job = &snap.wifi.job;
    if job.running() {
        let msg = if job.message.is_empty() {
            "Starting".to_string()
        } else {
            capital(&job.message)
        };
        v.row(info(what, msg));
    } else if let Some((ok, m)) = &job.finished {
        v.row(info(what, capital(m)).alert(!ok));
    }
}

fn wifi(snap: &Snapshot) -> View {
    let mut v = View::new(Layout::List, "Wi-Fi networks", true);
    if let Some(n) = &snap.wifi.note {
        v.row(info("Problem", n.clone()).alert(true));
    }
    match &snap.wifi.networks {
        None => {
            v.row(info("Asking carpid", "").dimmed(true));
        }
        Some(nets) => {
            for (i, n) in nets.iter().enumerate() {
                let value = if snap.wifi.connected.as_deref() == Some(n.ssid.as_str()) {
                    "Connected"
                } else {
                    ""
                };
                v.row(route(n.ssid.clone(), value, S::WifiNet(i)).icon("WifiOutlined"));
            }
            if nets.is_empty() {
                v.row(info("None saved", "").dimmed(true));
            }
        }
    }
    v.row(route("Add network", "", S::WifiAdd));
    wifi_job_row(&mut v, snap, "Last test");
    v.finish()
}

fn wifi_net(snap: &Snapshot, i: usize, forgetting: bool) -> View {
    let Some(n) = snap.wifi.networks.as_ref().and_then(|n| n.get(i)) else {
        let mut v = View::new(Layout::List, "Wi-Fi network", true);
        v.row(info("This network is gone", "").dimmed(true));
        return v.finish();
    };
    let mut v = View::new(Layout::List, &n.ssid, true);
    v.row(info(
        "Security",
        if n.secured { "Password" } else { "Open" },
    ));
    if n.hidden {
        v.row(info("Hidden", "Yes"));
    }
    v.row(
        Row::new(RowKind::Button, "Test", "")
            .tag("Drops hotspot")
            .button("Test")
            .act(Act::Wifi(WifiAct::Test(n.ssid.clone()))),
    );
    wifi_job_row(&mut v, snap, "Test");
    if n.secured {
        v.row(
            Row::new(RowKind::Button, "Change password", "")
                .button("Type")
                .act(Act::Type(TypeFor::WifiPassword(n.ssid.clone()))),
        );
    }
    let label = if forgetting {
        "Confirm forget"
    } else {
        "Forget"
    };
    v.row(
        Row::new(RowKind::Button, label, "")
            .button("Forget")
            .act(Act::Wifi(WifiAct::Forget(n.ssid.clone())))
            .alert(forgetting),
    );
    v.finish()
}

fn wifi_add(snap: &Snapshot) -> View {
    let mut v = View::new(Layout::List, "Add network", true);
    v.row(
        Row::new(RowKind::Button, "Scan", "")
            .tag("Drops hotspot ~10 s")
            .button("Scan")
            .act(Act::Wifi(WifiAct::Scan)),
    );
    wifi_job_row(&mut v, snap, "Scan");
    if let Some(seen) = &snap.wifi.seen {
        for s in seen {
            let value = format!("{} %{}", s.signal, if s.saved { " · saved" } else { "" });
            let act = if s.secured {
                Act::Type(TypeFor::WifiPassword(s.ssid.clone()))
            } else {
                Act::Wifi(WifiAct::AddOpen(s.ssid.clone()))
            };
            v.row(
                Row::new(RowKind::Route, s.ssid.clone(), value)
                    .act(act)
                    .icon("WifiOutlined"),
            );
        }
    }
    v.row(
        Row::new(RowKind::Route, "Hidden network", "")
            .act(Act::Type(TypeFor::WifiName)),
    );
    v.finish()
}

fn update_confirm(snap: &Snapshot) -> View {
    let mut v = View::new(Layout::List, "Install", true).note(settings_note(snap));
    v.row(info("Hotspot drops during download", ""));
    let at = v.row(
        Row::new(RowKind::Button, "Install", "")
            .button("Install")
            .act(Act::Update),
    );
    v.row(
        Row::new(RowKind::Button, "Cancel", "")
            .button("Cancel")
            .act(Act::Back),
    );
    v.start = at;
    v.finish()
}

fn advanced(pages: &Pages, snap: &Snapshot) -> View {
    with_settings(snap, "Advanced", |v, s| {
        if s.flashing_mode {
            v.row(info("Flashing mode is on", "CAN sends blocked").alert(true));
        }
        if snap.storage.can_move() {
            v.row(route("Move to NVMe", "Running from the SD", S::Nvme).icon("MemoryOutlined"));
        }
        if pages.setup_done() {
            v.row(
                Row::new(RowKind::Button, "Show Setup", "")
                    .button("Show")
                    .act(Act::SetupDone(false)),
            );
        }
        v.row(toggle(
            "CAN transmit",
            s.tx_enabled,
            set(SettingsPatch {
                tx_enabled: Some(!s.tx_enabled),
                ..Default::default()
            }),
        ));
        v.row(toggle(
            "Key input",
            s.input.uinput,
            input(InputPatch {
                uinput: Some(!s.input.uinput),
                ..Default::default()
            }),
        ));
    })
}

// ---- Acts ----

impl Pages {
    /// Runs a tab-only act; `None` when it isn't one.
    pub(super) fn tab_run(&mut self, act: &Act, client: &Client) -> Option<Moved> {
        if !matches!(act, Act::Dis(DisAct::RemovePage(_))) {
            self.removing = false;
        }
        if !matches!(act, Act::Wifi(WifiAct::Forget(_))) {
            self.forgetting = false;
        }
        if *act != Act::MoveNvme {
            self.moving = false;
        }
        if !matches!(act, Act::FirmwareInstall(_)) {
            self.flashing = None;
        }
        if !matches!(act, Act::FlashSector(_)) {
            self.flashing_sector = None;
        }
        // A screen Setup opens comes back to Setup.
        if let (S::Setup, Act::Car(to)) = (self.car, act) {
            self.via_setup = Some(*to);
        }
        // Opening the Wi-Fi screens asks for the saved networks; Setup, Advanced and Move to
        // NVMe ask where the system runs from.
        if matches!(act, Act::Car(S::Wifi | S::Setup)) {
            client.wifi(carpi_proto::WifiAction::List);
        }
        if matches!(act, Act::Car(S::Setup | S::Advanced | S::Nvme)) {
            client.storage(carpi_proto::StorageAction::Status);
        }
        if matches!(act, Act::Car(S::Updates)) {
            client.update_status();
        }
        // The firmware screen asks carpid's status, and probes unless a job ran just now.
        if matches!(act, Act::Car(S::Firmware | S::AvsdFirmware)) {
            client.firmware_opened();
        }
        Some(match act {
            Act::Dis(a) => self.dis_run(a, client),
            Act::Zone(z) => {
                client.set(SettingsPatch {
                    clock: Some(ClockPatch {
                        timezone: Some(z.clone()),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                self.up(2)
            }
            Act::Update => {
                client.update(UpdateAction::Fetch, true);
                self.up(1)
            }
            Act::Type(what) => {
                self.type_for(what.clone(), client);
                Moved::Stay
            }
            Act::Keyboard(prefs) => {
                self.set_keyboard_prefs(*prefs);
                Moved::Stay
            }
            Act::SetupDone(done) => {
                self.set_setup_done(*done);
                if *done { self.up(1) } else { Moved::Stay }
            }
            Act::FirmwareProbe => {
                client.firmware(carpi_proto::FirmwareAction::Probe);
                Moved::Stay
            }
            // Same as Move to NVMe: the first push shows what happens, the second starts it.
            Act::FirmwareInstall(unit) => {
                if self.flashing != Some(*unit) {
                    self.flashing = Some(*unit);
                    return Some(Moved::Stay);
                }
                self.flashing = None;
                client.firmware(unit.install());
                Moved::Stay
            }
            Act::FlashSector(name) => {
                if self.flashing_sector.as_deref() != Some(name.as_str()) {
                    self.flashing_sector = Some(name.clone());
                    return Some(Moved::Stay);
                }
                self.flashing_sector = None;
                client.firmware(carpi_proto::FirmwareAction::FlashSector {
                    name: name.clone(),
                    confirm: true,
                });
                Moved::Stay
            }
            Act::MoveNvme => {
                if !self.moving {
                    self.moving = true;
                    return Some(Moved::Stay);
                }
                self.moving = false;
                client.storage(carpi_proto::StorageAction::MoveToNvme);
                Moved::Stay
            }
            Act::Wifi(a) => {
                use carpi_proto::WifiAction as W;
                match a {
                    WifiAct::List => client.wifi(W::List),
                    WifiAct::Scan => client.wifi(W::Scan),
                    WifiAct::AddOpen(ssid) => {
                        client.wifi(W::Add {
                            ssid: ssid.clone(),
                            password: None,
                            hidden: false,
                        });
                        return Some(self.up(1));
                    }
                    WifiAct::Test(ssid) => client.wifi(W::Test { ssid: ssid.clone() }),
                    WifiAct::Forget(ssid) => {
                        if !self.forgetting {
                            self.forgetting = true;
                            return Some(Moved::Stay);
                        }
                        self.forgetting = false;
                        client.wifi(W::Forget { ssid: ssid.clone() });
                        return Some(self.up(1));
                    }
                }
                Moved::Stay
            }
            Act::Back => self.up(1),
            _ => return None,
        })
    }

    /// Back `n` screens; focus where it was on the last.
    fn up(&mut self, n: usize) -> Moved {
        let mut moved = Moved::Stay;
        for _ in 0..n {
            match self.back(crate::PageId::Car) {
                Some(m) => moved = m,
                None => break,
            }
        }
        moved
    }

    fn dis_run(&mut self, a: &DisAct, client: &Client) -> Moved {
        let Some(s) = client.snapshot().settings.clone() else {
            return Moved::Stay;
        };
        let mut pages = cfg::dis_pages(&s.dis);
        let mut moved = Moved::Stay;
        match a {
            DisAct::AddPage => {
                if pages.len() >= DIS_PAGES {
                    return Moved::Stay;
                }
                pages.push(DisPageSetting {
                    name: cfg::free_name(&pages),
                    cells: Vec::new(),
                });
                // Back lands on the new page's row (LIVI's back button comes first).
                self.trail.push(pages.len() as i32);
                self.car = S::ReadingPage(pages.len() - 1);
                moved = Moved::Opened;
            }
            DisAct::RemovePage(p) => {
                if !self.removing {
                    self.removing = true;
                    return Moved::Stay;
                }
                self.removing = false;
                if *p < pages.len() && pages.len() > 1 {
                    pages.remove(*p);
                    moved = self.up(1);
                }
            }
            DisAct::MovePage(p, to) => {
                if *p >= pages.len() || *to >= pages.len() || p == to {
                    return Moved::Stay;
                }
                let page = pages.remove(*p);
                pages.insert(*to, page);
                if self.car == S::ReadingPage(*p) {
                    self.car = S::ReadingPage(*to);
                } else {
                    moved = Moved::Focus(*to as i32);
                }
            }
            DisAct::Name(p, d) => match cfg::page_name_step(&pages, *p, *d) {
                Some(n) => pages[*p].name = n,
                None => return Moved::Stay,
            },
            DisAct::MoveCell(p, c, d) => {
                let Some(page) = pages.get_mut(*p) else {
                    return Moved::Stay;
                };
                let to = *c as i32 + d;
                if to < 0 || to as usize >= page.cells.len() {
                    return Moved::Stay;
                }
                page.cells.swap(*c, to as usize);
                // The name row comes first.
                moved = Moved::Focus(to + 1);
            }
            DisAct::Decimals(p, c, x) => match cell_mut(&mut pages, *p, *c) {
                Some(cell) => cell.decimals = *x,
                None => return Moved::Stay,
            },
            DisAct::Unit(p, c, u) => match cell_mut(&mut pages, *p, *c) {
                Some(cell) => cell.unit = u.clone(),
                None => return Moved::Stay,
            },
            DisAct::Icon(p, c, i) => {
                match cell_mut(&mut pages, *p, *c) {
                    Some(cell) => cell.icon = i.clone(),
                    None => return Moved::Stay,
                }
                moved = self.up(1);
            }
            DisAct::RemoveCell(p, c) => {
                let Some(page) = pages.get_mut(*p) else {
                    return Moved::Stay;
                };
                if *c < page.cells.len() {
                    page.cells.remove(*c);
                }
                moved = self.up(1);
            }
        }
        client.set(SettingsPatch {
            dis: Some(DisPatch {
                pages: Some(pages),
                ..Default::default()
            }),
            ..Default::default()
        });
        moved
    }

    /// The picker of a readings cell.
    pub(super) fn cell_pick(&mut self, a: &SlotAct, client: &Client) -> Moved {
        let (p, c) = match self.car {
            S::CellValue(p, c) => (p, Some(c)),
            S::CellAdd(p) => (p, None),
            _ => return Moved::Stay,
        };
        let name = match self.reading.apply(a, &slots::DIS, &self.saved.recent) {
            Picked::Moved => return Moved::Opened,
            Picked::Stay => return Moved::Stay,
            Picked::Chose(name) => name,
            Picked::ChoseAll(names) => names.into_iter().next(),
        };
        let Some(s) = client.snapshot().settings.clone() else {
            return Moved::Stay;
        };
        if let Some(n) = &name {
            slots::remember(&mut self.saved.recent, n);
            self.store();
        }
        let mut pages = cfg::dis_pages(&s.dis);
        let Some(page) = pages.get_mut(p) else {
            return Moved::Stay;
        };
        let mut levels = 1;
        match (c, name) {
            (Some(c), Some(n)) => {
                if let Some(cell) = page.cells.get_mut(c)
                    && cell.key != n
                {
                    *cell = cfg::new_cell(&n);
                }
            }
            (Some(c), None) => {
                if c < page.cells.len() {
                    page.cells.remove(c);
                }
                levels = 2;
            }
            (None, Some(n)) => {
                if page.cells.len() < DIS_CELLS {
                    page.cells.push(cfg::new_cell(&n));
                }
            }
            (None, None) => {}
        }
        client.set(SettingsPatch {
            dis: Some(DisPatch {
                pages: Some(pages),
                ..Default::default()
            }),
            ..Default::default()
        });
        self.reading = Picker::default();
        self.up(levels)
    }
}

fn cell_mut(
    pages: &mut [DisPageSetting],
    p: usize,
    c: usize,
) -> Option<&mut carpi_proto::DisCellSetting> {
    pages.get_mut(p)?.cells.get_mut(c)
}

/// Builder tweak in place.
trait Tap: Sized {
    fn tap(self, f: impl FnOnce(&mut Self)) -> Self;
}

impl Tap for Row {
    fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Saved;
    use crate::view::Hit;

    fn snap() -> Snapshot {
        let mut s = Snapshot {
            connected: true,
            problem: None,
            update_recovering: false,
            ..Snapshot::default()
        };
        s.settings = Some(cfg::sample());
        s
    }

    fn at(p: &mut Pages, screen: S) -> View {
        p.car = screen;
        view(p, &snap(), Instant::now())
    }

    fn find(v: &View, label: &str) -> usize {
        v.rows
            .iter()
            .position(|r| r.label == label)
            .unwrap_or_else(|| panic!("no row {label}: {:?}", v.rows))
    }

    fn patch_of(act: Option<&Act>) -> SettingsPatch {
        match act {
            Some(Act::Patch(p, _)) => (**p).clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn last_update_is_one_row_and_a_new_failure_replaces_old_history() {
        let mut s = snap();
        s.update_status = Some(carpi_proto::Installed {
            current: Some("beta-20261109-5e0cf2c".into()),
            previous: Some("8f2a408".into()),
            last_apply: Some("ok".into()),
            update_running: Some(false),
            ..Default::default()
        });
        let v = updates(&s);
        assert_eq!(
            v.rows[find(&v, "Version")].value,
            "beta-20261109-5e0cf2c"
        );
        assert_eq!(v.rows[find(&v, "Last update")].value, "Succeeded");
        assert!(!v.rows[find(&v, "Last update")].alert);
        assert!(v.rows[find(&v, "Install")].value.is_empty());
        // A refused update doesn't sit beside an old "Succeeded".
        s.update.finished = Some((false, "needs OS profile 2, Pi has 1".into()));
        let v = updates(&s);
        let row = &v.rows[find(&v, "Last update")];
        assert_eq!(row.value, "Failed: needs OS profile 2, Pi has 1");
        assert!(row.alert);
        assert_eq!(
            v.rows
                .iter()
                .filter(|r| r.value.contains("ucceeded"))
                .count(),
            0
        );
        assert!(v.rows[find(&v, "Install")].value.is_empty());
    }

    #[test]
    fn passive_status_polling_is_not_an_update_job() {
        let mut s = snap();
        s.update_recovering = true;
        let v = updates(&s);
        assert_eq!(
            v.rows[find(&v, "Status")].value,
            "Checking"
        );
        assert!(!v.rows[find(&v, "Status")].alert);
        assert!(!v.rows.iter().any(|r| r.label == "Updating"));
        assert!(!s.update.running());
    }

    #[test]
    fn unknown_update_outcome_is_informational_and_failures_are_alerts() {
        let mut s = snap();
        s.update_note = Some("Update outcome unknown; running 8f2a408".into());
        let v = updates(&s);
        let row = &v.rows[find(&v, "Last update")];
        assert_eq!(row.value, "Update outcome unknown; running 8f2a408");
        assert!(!row.alert);
        for (outcome, history, message) in [
            (
                "rolled_back",
                "Rolled back",
                "Update rolled back; running 8f2a408",
            ),
            ("failed", "Failed", "Update failed; running 8f2a408"),
        ] {
            s.update_note = None;
            s.update_status = Some(carpi_proto::Installed {
                last_apply: Some(outcome.into()),
                update_running: Some(false),
                ..Default::default()
            });
            // A fresh UI still warns about authoritative failure history.
            let v = updates(&s);
            let row = &v.rows[find(&v, "Last update")];
            assert_eq!(row.value, history);
            assert!(row.alert);
            // The same outcome for a locally observed attempt remains an alert too.
            s.update.finished = Some((false, message.into()));
            let v = updates(&s);
            let row = &v.rows[find(&v, "Last update")];
            assert_eq!(row.value, format!("Failed: {message}"));
            assert!(row.alert);
            s.update.finished = None;
        }
    }

    #[test]
    fn flashing_mode_is_on_the_root_and_preserves_the_send_preference() {
        let p = Pages::new(Saved::default(), None);
        let mut snap = snap();
        let v = view(&p, &snap, Instant::now());
        let row = &v.rows[find(&v, "Flashing mode")];
        let patch = patch_of(Some(&row.act));
        assert_eq!(patch.flashing_mode, Some(true));
        assert_eq!(patch.tx_enabled, None);
        cfg::apply(snap.settings.as_mut().unwrap(), &patch);
        let v = view(&p, &snap, Instant::now());
        let row = &v.rows[find(&v, "Flashing mode")];
        assert!(row.checked && row.alert);
        assert_eq!(row.tag, "CAN sends blocked");
        assert_eq!(patch_of(Some(&row.act)).flashing_mode, Some(false));
        assert!(!snap.settings.as_ref().unwrap().tx_enabled);
    }

    #[test]
    fn every_screen_builds_and_every_focusable_row_does_something() {
        let mut p = Pages::new(Saved::default(), None);
        for screen in [
            S::Root,
            S::Status,
            S::AllValues,
            S::Buses,
            S::Cluster,
            S::ReadingPages,
            S::ReadingPage(0),
            S::Cell(0, 0),
            S::CellIcon(0, 0),
            S::Nav,
            S::Calls,
            S::Media,
            S::Accel,
            S::Lighting,
            S::Curve,
            S::Point(0),
            S::Night,
            S::Controls,
            S::Wheel,
            S::Faceplate,
            S::Key(0),
            S::Binding(0, 1),
            S::Logs,
            S::Power,
            S::Zones,
            S::Zone(0),
            S::Avsd,
            S::Picture,
            S::Pin,
            S::Updates,
            S::Firmware,
            S::AvsdFirmware,
            S::Remote,
            S::UpdateConfirm,
            S::Advanced,
            S::Setup,
            S::Nvme,
        ] {
            let v = at(&mut p, screen);
            assert!(!v.title.is_empty(), "{screen:?}");
            for r in &v.rows {
                let acts = r.act != Act::None || r.side.is_some();
                let plain = matches!(r.kind, RowKind::Info | RowKind::Section);
                assert!(acts || plain, "{screen:?}: {} does nothing", r.label);
                if matches!(r.kind, RowKind::Choice | RowKind::Step) {
                    assert!(r.act != Act::None, "{screen:?}: push on {}", r.label);
                    assert!(r.side.is_some(), "{screen:?}: D-pad on {}", r.label);
                }
                assert!(!r.label.contains('_'), "{screen:?}: {}", r.label);
            }
        }
    }

    #[test]
    fn the_tree_goes_back_the_way_it_came() {
        assert_eq!(S::Cell(1, 2).parent(), Some(S::ReadingPage(1)));
        assert_eq!(S::CellValue(1, 2).parent(), Some(S::Cell(1, 2)));
        assert_eq!(S::Binding(0, 4).parent(), Some(S::Key(0)));
        assert_eq!(S::Key(0).parent(), Some(S::Wheel));
        assert_eq!(
            S::Key(keys::key_at("enter").unwrap()).parent(),
            Some(S::Faceplate)
        );
        assert_eq!(S::Picture.depth(), 2);
        assert!(S::Night.light() && !S::Cluster.light());
    }

    #[test]
    fn cluster_rows_are_terse_and_routes_show_no_setting() {
        let mut p = Pages::new(Saved::default(), None);
        let v = at(&mut p, S::Cluster);
        let claim = &v.rows[find(&v, "Auto claim")];
        assert!(claim.checked);
        assert_eq!(
            patch_of(Some(&claim.act)).dis.unwrap().claim,
            Some(DisClaim::Tab)
        );
        for r in v.rows.iter().filter(|r| r.kind == RowKind::Route) {
            assert!(r.value.is_empty(), "{}: {}", r.label, r.value);
        }
        for (screen, flag) in [(S::Nav, 0), (S::Calls, 1), (S::Media, 2)] {
            let v = at(&mut p, screen);
            let take = &v.rows[find(&v, "Auto claim")];
            assert!(take.checked);
            let d = patch_of(Some(&take.act)).dis.unwrap();
            let got = [d.claim_nav, d.claim_phone, d.claim_media][flag];
            assert_eq!(got, Some(false), "{screen:?}");
            find(&v, "Auto release");
        }
        let v = at(&mut p, S::Nav);
        assert_eq!(v.rows[find(&v, "Style")].value, "Fancy");
        find(&v, "ETA");
        assert_eq!(v.rows[find(&v, "Lanes")].value, "Alternate");
        let v = at(&mut p, S::Media);
        let flash = find(&v, "Fullscreen cover time");
        assert_eq!(v.rows[flash].value, "4 s");
        assert_eq!(
            patch_of(Some(&v.rows[flash].act))
                .dis
                .unwrap()
                .cover_flash_s,
            Some(5.0)
        );
    }

    #[test]
    fn readings_pages_edit_through_carpid() {
        let client = crate::client::Fixture {
            settings: Some(cfg::sample()),
            ..Default::default()
        }
        .start();
        let mut p = Pages::new(Saved::default(), None);
        p.car = S::ReadingPage(0);
        p.trail = vec![0, 0, 0];
        let s = client.snapshot().clone();
        let v = view(&p, &s, Instant::now());
        assert_eq!(v.title, "Readings");
        let add = find(&v, "Add value");
        let m = p.press(crate::PageId::Car, &v, &Hit::Row(add), 3, &client);
        assert_eq!((m, p.car), (Moved::Opened, S::CellAdd(0)));
        let m = p.run(
            crate::PageId::Car,
            Act::Slots(SlotAct::Pick("engine.rpm".into())),
            0,
            &client,
        );
        assert_eq!((m, p.car), (Moved::Closed(3), S::ReadingPage(0)));
        let wait = || {
            for _ in 0..200 {
                if client
                    .snapshot()
                    .settings
                    .as_ref()
                    .is_some_and(|s| s.dis.pages.first().is_some_and(|p| p.cells.len() == 2))
                {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        assert!(wait(), "the page got the value");
        let pages = client.snapshot().settings.clone().unwrap().dis.pages;
        assert_eq!(pages[0].cells[1].key, "engine.rpm");
        assert_eq!(pages[0].name, "Readings");
    }

    #[test]
    fn a_page_is_removed_on_the_second_push() {
        let mut p = Pages::new(Saved::default(), None);
        let client = Client::offline("test");
        assert_eq!(
            p.tab_run(&Act::Dis(DisAct::RemovePage(0)), &client),
            Some(Moved::Stay)
        );
        assert!(!p.removing, "no settings, nothing to ask");
        assert_eq!(clock_step("23:45", 30), "00:15");
        assert_eq!(clock_step("07:00", -30), "06:30");
    }

    #[test]
    fn avsd_rows_step_in_range() {
        let r = AvsdRange {
            key: "KEYINT".into(),
            label: "Key frame every".into(),
            unit: "frames".into(),
            min: 10.0,
            max: 120.0,
            step: 10.0,
            default: 60.0,
            zero_off: true,
            help: String::new(),
        };
        let row = avsd_row(&r, 10.0);
        let val = |a: &Act| patch_of(Some(a)).avsd.unwrap().values["KEYINT"];
        let (l, rr) = row.side.clone().unwrap();
        assert_eq!((val(&l), val(&rr)), (0.0, 20.0));
        assert_eq!(avsd_text(&r, 0.0), "Off");
        assert_eq!(avsd_text(&r, 60.0), "60 frames");
        let on = AvsdRange {
            key: "DITHER".into(),
            min: 0.0,
            max: 1.0,
            step: 1.0,
            zero_off: false,
            ..r
        };
        assert_eq!(avsd_row(&on, 1.0).kind, RowKind::Toggle);
    }

    #[test]
    fn decoder_row_reads_and_displays_progress_crc_and_errors() {
        let mut s = snap();
        let row = |s: &Snapshot| {
            let v = avsd(s, Instant::now());
            v.rows[find(&v, "Legacy decoder CRC")].clone()
        };
        assert_eq!(row(&s).act, Act::DecoderCrc);
        assert_eq!(row(&s).button, "Read CRC");
        assert_eq!(row(&s).tag, "Diagnostic only");
        assert_eq!(row(&s).value, "Sector 107 CRC32");
        s.decoder.asked = true;
        assert_eq!(row(&s).act, Act::None);
        assert!(row(&s).value.contains("Reading"));
        s.decoder.progress = Some((65536, Some(131072)));
        assert!(row(&s).value.contains("50%"));
        s.decoder.finished = Some((true, "0696DDE4".into()));
        assert_eq!(row(&s).value, "CRC32 0696DDE4");
        assert_eq!(s.firmware, crate::firmware::Status::default());
        assert!(!row(&s).alert);
        s.decoder.finished = Some((false, "RNS-E did not reply".into()));
        assert!(row(&s).alert);
        assert_eq!(row(&s).value, "RNS-E did not reply");
        assert_eq!(row(&s).act, Act::DecoderCrc);
    }

    #[test]
    fn firmware_install_needs_a_second_push() {
        use crate::firmware::{Comparison, Identity, Installed, Target, Unit};
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let mut s = snap();
        let target = Target {
            installed: Installed::Reported(Identity {
                version: "1".into(),
                build_id: None,
            }),
            bundled: Some(Identity {
                version: "2".into(),
                build_id: None,
            }),
            comparison: Comparison::Newer,
            ..Target::default()
        };
        s.firmware.rnse = target.clone();
        s.firmware.teensy = target;
        p.goto(S::Firmware);
        let press = |p: &mut Pages, label: &str, nth: usize| {
            let v = view(p, &s, Instant::now());
            let at = v
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.label == label)
                .nth(nth)
                .map(|(i, _)| i)
                .unwrap_or_else(|| panic!("no {label}"));
            p.press(crate::PageId::Car, &v, &Hit::Row(at), 0, &client)
        };
        assert_eq!(press(&mut p, "Install update", 0), Moved::Stay);
        assert_eq!(p.flashing(), Some(Unit::Rnse));
        assert_eq!(
            client.snapshot().firmware_job.action,
            None,
            "nothing sent yet"
        );
        let v = view(&p, &s, Instant::now());
        assert!(v.rows.iter().any(|r| r.label == "Ignition on, engine off"));
        // The other unit's button re-arms for that unit instead of installing.
        press(&mut p, "Install update", 0);
        assert_eq!(p.flashing(), Some(Unit::Teensy));
        assert_eq!(client.snapshot().firmware_job.action, None);
        // Anything else disarms.
        press(&mut p, "Check versions", 0);
        assert_eq!(p.flashing(), None);
        client.snapshot().firmware_job = Default::default();
        press(&mut p, "Install update", 0);
        assert_eq!(press(&mut p, "Confirm install", 0), Moved::Stay);
        assert_eq!(p.flashing(), None);
        assert_eq!(
            client.snapshot().firmware_job.action,
            Some(carpi_proto::FirmwareAction::InstallRnse { confirm: true })
        );
        // Leaving the screen disarms too.
        client.snapshot().firmware_job = Default::default();
        press(&mut p, "Install update", 1);
        assert_eq!(p.flashing(), Some(Unit::Teensy));
        p.back(crate::PageId::Car);
        assert_eq!(p.flashing(), None);
    }

    #[test]
    fn firmware_routes_return_to_the_screen_that_opened_them() {
        let client = Client::offline("test");
        let mut p = Pages::new(Saved::default(), None);
        let s = snap();
        for (parent, screen) in [(S::Updates, S::Firmware), (S::Avsd, S::AvsdFirmware)] {
            p.goto(parent);
            let v = view(&p, &s, Instant::now());
            let at = find(&v, "SDLink firmware");
            assert_eq!(
                p.press(crate::PageId::Car, &v, &Hit::Row(at), 2, &client),
                Moved::Opened
            );
            assert_eq!(p.car, screen);
            let v = view(&p, &s, Instant::now());
            assert_eq!(v.title, "SDLink firmware");
            assert_eq!(p.back(crate::PageId::Car), Some(Moved::Closed(2)));
            assert_eq!(p.car, parent);
        }
        // The native Software Update slot has Updates as its root.
        p.open_updates(&client);
        let v = view(&p, &s, Instant::now());
        let at = find(&v, "SDLink firmware");
        p.press(crate::PageId::Car, &v, &Hit::Row(at), 2, &client);
        p.back(crate::PageId::Car);
        assert_eq!(p.car, S::Updates);
        assert_eq!(p.back(crate::PageId::Car), None);
    }

    fn nvme_snap(root: carpi_proto::RootDisk) -> Snapshot {
        let mut s = snap();
        s.storage.status = Some(carpi_proto::StorageStatus {
            root: "/dev/mmcblk0p2".into(),
            root_disk: root,
            nvme: Some(carpi_proto::NvmeDisk {
                path: "/dev/nvme0n1".into(),
                model: "WD Blue SN580".into(),
                size: 500_107_862_016,
            }),
            used: 6_100_000_000,
            blocked: None,
        });
        s
    }

    #[test]
    fn setup_is_separate_from_car_integration_and_its_screens_come_back_to_it() {
        let mut p = Pages::new(Saved::default(), None);
        let client = Client::offline("test");
        let s = snap();
        let v = view(&p, &s, Instant::now());
        assert!(!v.rows.iter().any(|r| r.label == "Setup"));
        p.open_setup(&client);
        assert_eq!(p.car, S::Setup);
        let v = view(&p, &s, Instant::now());
        let phone = find(&v, "Phone connection");
        assert_eq!(v.rows[phone].act, Act::PhoneSettings);
        assert_eq!(v.hit(v.rows[phone].focus_at), Some(Hit::Row(phone)));
        assert!(
            v.rows
                .iter()
                .any(|r| r.label.contains("supported authentication hardware"))
        );
        // No NVMe reported: no move row.
        assert!(!v.rows.iter().any(|r| r.label == "Move to NVMe"));
        let remote = find(&v, "Remote access");
        assert!(v.rows[remote].value.is_empty());
        p.press(crate::PageId::Car, &v, &Hit::Row(remote), 4, &client);
        assert_eq!(p.car, S::Remote);
        assert_eq!(p.depth(crate::PageId::Car), 2);
        assert_eq!(p.back(crate::PageId::Car), Some(Moved::Closed(4)));
        assert_eq!(p.car, S::Setup);
        // The time zone: picking one goes back two, to Setup.
        let v = view(&p, &s, Instant::now());
        let zone = find(&v, "Time zone");
        p.press(crate::PageId::Car, &v, &Hit::Row(zone), 1, &client);
        assert_eq!(p.car, S::Zones);
        p.run(crate::PageId::Car, Act::Car(S::Zone(0)), 0, &client);
        assert_eq!(p.depth(crate::PageId::Car), 3);
        p.run(
            crate::PageId::Car,
            Act::Zone("Europe/Berlin".into()),
            0,
            &client,
        );
        assert_eq!(p.car, S::Setup);
        // Back from Setup's own screens' parents is untouched elsewhere.
        p.goto(S::Remote);
        p.back(crate::PageId::Car);
        assert_eq!(p.car, S::Root);
        // Hide persists the main Settings preference; Advanced brings it back.
        p.goto(S::Setup);
        let v = view(&p, &s, Instant::now());
        let hide = find(&v, "Finish setup");
        p.press(crate::PageId::Car, &v, &Hit::Row(hide), 0, &client);
        assert_eq!(p.car, S::Root);
        assert!(p.setup_done());
        let v = view(&p, &s, Instant::now());
        assert!(!v.rows.iter().any(|r| r.label == "Setup"));
        p.goto(S::Advanced);
        let v = view(&p, &s, Instant::now());
        let show = find(&v, "Show Setup");
        p.press(crate::PageId::Car, &v, &Hit::Row(show), 0, &client);
        assert!(!p.setup_done());
    }

    #[test]
    fn move_to_nvme_shows_only_from_the_sd_and_needs_two_pushes() {
        let mut p = Pages::new(Saved::default(), None);
        let client = Client::offline("test");
        let on_nvme = nvme_snap(carpi_proto::RootDisk::Nvme);
        p.car = S::Setup;
        assert!(
            !view(&p, &on_nvme, Instant::now())
                .rows
                .iter()
                .any(|r| r.label == "Move to NVMe")
        );
        let s = nvme_snap(carpi_proto::RootDisk::Sd);
        let v = view(&p, &s, Instant::now());
        let mv = find(&v, "Move to NVMe");
        p.press(crate::PageId::Car, &v, &Hit::Row(mv), 0, &client);
        assert_eq!(p.car, S::Nvme);
        p.goto(S::Advanced);
        assert!(
            view(&p, &s, Instant::now())
                .rows
                .iter()
                .any(|r| r.label == "Move to NVMe")
        );
        p.goto(S::Nvme);
        let v = view(&p, &s, Instant::now());
        assert_eq!(v.rows[0].value, "WD Blue SN580, 500 GB");
        let start = find(&v, "Start the move");
        assert_eq!(v.start, v.rows[start].focus_at);
        p.press(crate::PageId::Car, &v, &Hit::Row(start), 0, &client);
        assert!(p.moving());
        assert!(
            client.snapshot().storage.job.finished.is_none(),
            "not on the first push"
        );
        let v = view(&p, &s, Instant::now());
        let again = find(&v, "Confirm erase");
        p.press(crate::PageId::Car, &v, &Hit::Row(again), 0, &client);
        assert!(!p.moving());
        // Offline, so the ask comes straight back refused.
        assert_eq!(
            client.snapshot().storage.job.finished,
            Some((false, "not connected".into()))
        );
        // Blocked: says why, no start.
        let mut b = s.clone();
        b.storage.status.as_mut().unwrap().blocked = Some("The NVMe is smaller".into());
        let v = view(&p, &b, Instant::now());
        assert!(!v.rows.iter().any(|r| r.act == Act::MoveNvme));
        assert_eq!(v.rows[find(&v, "Can't start")].value, "The NVMe is smaller");
        // Running: progress, no start.
        let mut r = s.clone();
        r.storage.job = crate::client::UpdateJob {
            job: Some(3),
            message: "copying the system".into(),
            progress: Some((37, Some(100))),
            ..Default::default()
        };
        let v = view(&p, &r, Instant::now());
        assert_eq!(v.rows[find(&v, "Moving")].value, "Copying the system 37 %");
        assert!(!v.rows.iter().any(|r| r.act == Act::MoveNvme));
    }
}
