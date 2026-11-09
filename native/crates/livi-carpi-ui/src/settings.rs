// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpid's settings as the pages edit them: what each binding table takes, short labels for the
//! cluster display, and a patch applied the way carpid applies it (for the fixture and tests).

use carpi_proto::{
    Binding, DisCellSetting, DisPageSetting, DisSettings, PinChange, ReadingSetting, Settings,
    SettingsPatch,
};

use crate::defs::find;

/// Readings on the cluster display's page.
pub const DIS_SLOTS: usize = 8;

/// Actions each binding table takes, as carpid names them (`carpi-input/src/names.rs`).
pub fn actions(table: &str) -> &'static [&'static str] {
    const SYSTEM: &[&str] = &[
        "toggle_dis_focus",
        "shutdown",
        "reboot",
        "save_capture",
        "brightness_up",
        "brightness_down",
        "toggle_logging",
    ];
    const CAR: &[&str] = &[
        "open_nav",
        "open_phone",
        "open_media",
        "open_page_data",
        "open_page_car",
        "call_accept",
        "call_end",
    ];
    const DIS: &[&str] = &["up", "down", "select", "back"];
    const LIVI: &[&str] = &[
        "up",
        "down",
        "left",
        "right",
        "select",
        "back",
        "home",
        "play_pause",
        "next",
        "prev",
        "accept_phone",
        "reject_phone",
        "voice",
        "cycle_session",
    ];
    match table {
        "dis_toggle" => &["toggle"],
        "system" => SYSTEM,
        "call_ringing" | "call_active" | "car" => CAR,
        "dis" => DIS,
        "livi" => LIVI,
        _ => &[],
    }
}

/// What a binding row is called on screen.
pub fn binding_label(b: &Binding) -> String {
    format!("{} · {} {}", b.table, b.key, b.gesture)
}

/// The table with row `at` doing `action`.
pub fn rebound(table: &[Binding], at: usize, action: &str) -> Vec<Binding> {
    let mut out = table.to_vec();
    if let Some(b) = out.get_mut(at) {
        b.action = action.to_string();
    }
    out
}

/// Cover flash lengths the D-pad steps through, seconds; 0 is off.
pub const COVER_FLASH: [f64; 8] = [0.0, 2.0, 3.0, 4.0, 5.0, 8.0, 10.0, 15.0];

/// The cover flash step `d` steps from `now`, stopping at the ends. A length off the list goes
/// to its neighbour.
pub fn flash_step(now: f64, d: i32) -> f64 {
    let at = if d > 0 {
        COVER_FLASH.iter().rposition(|x| *x <= now).unwrap_or(0) as i32 + d
    } else {
        COVER_FLASH
            .iter()
            .position(|x| *x >= now)
            .unwrap_or(COVER_FLASH.len()) as i32
            + d
    };
    COVER_FLASH[at.clamp(0, COVER_FLASH.len() as i32 - 1) as usize]
}

/// Values on one readings page.
pub const DIS_CELLS: usize = 8;
/// Readings pages the wheel steps through.
pub const DIS_PAGES: usize = 6;
/// Readings page names; others come from the portal.
pub const DIS_NAMES: &[&str] = &[
    "Engine",
    "Boost",
    "Haldex",
    "Electrical",
    "Trip",
    "Custom 1",
    "Custom 2",
    "Custom 3",
    "Custom 4",
];

/// The readings pages as carpid shows them: the named pages, or the old single page as one
/// named "Readings".
pub fn dis_pages(d: &DisSettings) -> Vec<DisPageSetting> {
    if !d.pages.is_empty() {
        return d.pages.clone();
    }
    if d.readings.is_empty() {
        return Vec::new();
    }
    vec![DisPageSetting {
        name: "Readings".into(),
        cells: d
            .readings
            .iter()
            .map(|r| DisCellSetting {
                key: r.key.clone(),
                decimals: r.decimals,
                scale: r.scale,
                unit: None,
                icon: None,
            })
            .collect(),
    }]
}

/// A new cell for `key`, with the value's own decimals.
pub fn new_cell(key: &str) -> DisCellSetting {
    let decimals = match find(key).map(|d| d.kind) {
        Some(crate::defs::Kind::Num(dp)) => dp,
        _ => 0,
    };
    DisCellSetting {
        key: key.to_string(),
        decimals,
        scale: 1.0,
        unit: None,
        icon: None,
    }
}

/// The name `d` steps from page `p`'s, skipping names other pages have.
pub fn page_name_step(pages: &[DisPageSetting], p: usize, d: i32) -> Option<String> {
    let now = &pages.get(p)?.name;
    let taken = |n: &str| pages.iter().enumerate().any(|(i, x)| i != p && x.name == n);
    let at = DIS_NAMES.iter().position(|x| x == now);
    let len = DIS_NAMES.len() as i32;
    let start = at.map_or(if d > 0 { -1 } else { len }, |a| a as i32);
    (1..=len)
        .map(|k| DIS_NAMES[(start + d * k).rem_euclid(len) as usize])
        .find(|n| !taken(n))
        .map(str::to_string)
}

/// A name for a new page.
pub fn free_name(pages: &[DisPageSetting]) -> String {
    DIS_NAMES
        .iter()
        .find(|n| !pages.iter().any(|p| p.name == **n))
        .unwrap_or(&"Custom")
        .to_string()
}

/// `v` in unit `from` as `to`; `None` without a conversion (carpi-core's `convert`).
pub fn convert(v: f64, from: &str, to: &str) -> Option<f64> {
    if from == to || to.is_empty() {
        return Some(v);
    }
    Some(match (from, to) {
        ("°C", "°F") => v * 9.0 / 5.0 + 32.0,
        ("°F", "°C") => (v - 32.0) * 5.0 / 9.0,
        ("km/h", "mph") => v / 1.609_344,
        ("mph", "km/h") => v * 1.609_344,
        ("km", "mi") => v / 1.609_344,
        ("mi", "km") => v * 1.609_344,
        ("bar", "psi") => v * 14.503_77,
        ("bar", "kPa") => v * 100.0,
        ("bar", "mbar") => v * 1000.0,
        ("mbar", "bar") => v / 1000.0,
        ("mbar", "psi") => v * 0.014_503_77,
        ("mbar", "kPa") => v / 10.0,
        ("kPa", "psi") => v * 0.145_037_7,
        ("kPa", "bar") => v / 100.0,
        ("L", "gal") => v / 3.785_41,
        ("Nm", "lb-ft") => v * 0.737_562,
        _ => return None,
    })
}

/// Units a cell can show a value in, from the key's own unit (carpi-core's `convert`).
pub fn units_for(own: &str) -> Vec<&'static str> {
    const PAIRS: &[(&str, &[&str])] = &[
        ("°C", &["°C", "°F"]),
        ("°F", &["°F", "°C"]),
        ("km/h", &["km/h", "mph"]),
        ("mph", &["mph", "km/h"]),
        ("km", &["km", "mi"]),
        ("mi", &["mi", "km"]),
        ("bar", &["bar", "psi", "kPa", "mbar"]),
        ("mbar", &["mbar", "bar", "psi", "kPa"]),
        ("kPa", &["kPa", "psi", "bar"]),
        ("L", &["L", "gal"]),
        ("Nm", &["Nm", "lb-ft"]),
    ];
    PAIRS
        .iter()
        .find(|(u, _)| *u == own)
        .map_or_else(Vec::new, |(_, list)| list.to_vec())
}

/// Icons the cluster draws for a reading (carpi-dis `reading_art::ICONS`).
pub const CELL_ICONS: &[&str] = &[
    "oil", "cool", "temperature", "boost", "intake", "air", "engine", "gear", "awd", "ambient",
    "fuel", "target", "spark", "inject", "valve", "voltage", "steering", "brake", "speed",
    "lambda", "pressure", "torque", "exhaust", "clutch", "pump", "timing", "distance", "level",
    "status", "fan", "throttle", "pedal", "sparkplug", "camshaft", "piston", "wheel", "abs",
    "traction", "compressor", "heater", "door", "hood", "lights", "wiper", "parking", "seatbelt",
    "rpm", "flow", "ignition", "current",
];

/// Time zones by region: `America/Denver` is in `America`. Regions in first-seen order of a
/// sorted list; zones without a slash go under "Other".
pub fn zone_regions(zones: &[String]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for z in zones {
        let region = z.split_once('/').map_or("Other", |(r, _)| r).to_string();
        match out.iter_mut().find(|(r, _)| *r == region) {
            Some((_, list)) => list.push(z.clone()),
            None => out.push((region, vec![z.clone()])),
        }
    }
    out
}

/// The step `d` from `now` through `list`, stopping at the ends; a value off the list goes to
/// its neighbour.
pub fn list_step<T: Copy + PartialOrd>(list: &[T], now: T, d: i32) -> T {
    let at = if d > 0 {
        list.iter().rposition(|x| *x <= now).map_or(0, |i| i as i32 + d)
    } else {
        list.iter()
            .position(|x| *x >= now)
            .unwrap_or(list.len()) as i32
            + d
    };
    list[at.clamp(0, list.len() as i32 - 1) as usize]
}

/// The next one in `list` after `now`, wrapping: what a push does to a stepped value.
pub fn list_next<T: Copy + PartialOrd>(list: &[T], now: T) -> T {
    let at = list.iter().rposition(|x| *x <= now).unwrap_or(0);
    list[(at + 1) % list.len()]
}

/// About three characters for the cluster display: the value's label, shortened.
pub fn dis_label(name: &str) -> String {
    let label = find(name).map_or_else(
        || name.rsplit('.').next().unwrap_or(name).to_string(),
        |d| d.label.to_string(),
    );
    label
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(3)
        .collect::<String>()
        .to_uppercase()
}

/// The cluster display's readings with slot `at` showing `name`, or emptied with `None`. A slot
/// past the end adds one.
pub fn reading_set(list: &[ReadingSetting], at: usize, name: Option<&str>) -> Vec<ReadingSetting> {
    let mut out = list.to_vec();
    match name {
        None if at < out.len() => {
            out.remove(at);
        }
        None => {}
        Some(n) => {
            let decimals = match find(n).map(|d| d.kind) {
                Some(crate::defs::Kind::Num(dp)) => dp,
                _ => 0,
            };
            let r = ReadingSetting {
                key: n.to_string(),
                label: dis_label(n),
                decimals,
                scale: 1.0,
            };
            if at < out.len() {
                if out[at].key != n {
                    out[at] = r;
                }
            } else if out.len() < DIS_SLOTS {
                out.push(r);
            }
        }
    }
    out
}

/// `patch` applied to `s`, as carpid does it (without the validation).
pub fn apply(s: &mut Settings, patch: &SettingsPatch) {
    if let Some(v) = patch.tx_enabled {
        s.tx_enabled = v;
    }
    if let Some(v) = patch.power_shutdown {
        s.power_shutdown = v;
        s.power.shutdown = v;
    }
    if let Some(p) = &patch.power {
        if let Some(v) = p.shutdown {
            s.power.shutdown = v;
            s.power_shutdown = v;
        }
        if let Some(v) = p.silent_s {
            s.power.silent_s = v;
        }
        if let Some(v) = p.radio_hold_min {
            s.power.radio_hold_min = v;
        }
    }
    if let Some(p) = &patch.logs {
        if let Some(v) = p.capture_minutes {
            s.logs.capture_minutes = v;
        }
        if let Some(v) = p.max_pct {
            s.logs.max_pct = v;
        }
    }
    if let Some(p) = &patch.clock {
        if let Some(v) = p.set_from_car {
            s.clock.set_from_car = v;
        }
        if let Some(v) = &p.timezone {
            s.clock.timezone = v.clone();
        }
    }
    if let Some(p) = &patch.input {
        if let Some(v) = p.uinput {
            s.input.uinput = v;
        }
        if let Some(v) = p.long_ms {
            s.input.long_ms = v;
        }
        if let Some(v) = p.extended_s {
            s.input.extended_s = v;
        }
    }
    if let Some(v) = patch.portal.as_ref().and_then(|p| p.protect_downloads) {
        s.portal.protect_downloads = v;
    }
    if let Some(v) = patch.update.as_ref().and_then(|p| p.channel) {
        s.update.channel = v;
    }
    if let Some(p) = &patch.avsd {
        for (k, v) in &p.values {
            s.avsd.values.insert(k.clone(), *v);
        }
    }
    if let Some(v) = patch.dis_enabled {
        s.dis.enabled = v;
    }
    if let Some(v) = &patch.dis_readings {
        s.dis.readings = v.clone();
    }
    if let Some(d) = &patch.dis {
        let m = &mut s.dis;
        if let Some(v) = d.claim {
            m.claim = v;
        }
        if let Some(v) = d.default_page {
            m.default_page = v;
        }
        if let Some(v) = d.claim_nav {
            m.claim_nav = v;
        }
        if let Some(v) = d.release_nav {
            m.release_nav = v;
        }
        if let Some(v) = d.claim_phone {
            m.claim_phone = v;
        }
        if let Some(v) = d.release_phone {
            m.release_phone = v;
        }
        if let Some(v) = d.claim_media {
            m.claim_media = v;
        }
        if let Some(v) = d.release_media {
            m.release_media = v;
        }
        if let Some(v) = d.media_layout {
            m.media_layout = v;
        }
        if let Some(v) = d.cover_flash_s {
            m.cover_flash_s = v;
        }
        if let Some(v) = d.units {
            m.units = v;
        }
        if let Some(v) = d.clock {
            m.clock = v;
        }
        if let Some(v) = &d.pages {
            m.pages = v.clone();
        }
        if let Some(v) = d.nav_style {
            m.nav_style = v;
        }
        if let Some(v) = d.nav_distance {
            m.nav_distance = v;
        }
        if let Some(v) = d.nav_pocket {
            m.nav_pocket = v;
        }
        if let Some(v) = d.nav_eta {
            m.nav_eta = v;
        }
        if let Some(v) = d.nav_time_left {
            m.nav_time_left = v;
        }
        if let Some(v) = d.nav_dest_distance {
            m.nav_dest_distance = v;
        }
        if let Some(v) = d.nav_lanes {
            m.nav_lanes = v;
        }
    }
    if let Some(v) = &patch.bindings {
        s.bindings = v.clone();
    }
    match &patch.portal_pin {
        Some(PinChange::Set { .. }) => s.portal_pin_set = true,
        Some(PinChange::Clear) => s.portal_pin_set = false,
        None => {}
    }
    if let Some(l) = &patch.light {
        let m = &mut s.light;
        if let Some(v) = l.auto_brightness {
            m.auto_brightness = v;
        }
        if let Some(v) = l.scroll_trim {
            m.scroll_trim = v;
        }
        if let Some(v) = l.ambient_source {
            m.ambient_source = v;
        }
        if let Some(v) = l.curve_mode {
            m.curve_mode = v;
        }
        if let Some(v) = &l.manual_curve {
            m.manual_curve = v.clone();
        }
        if let Some(v) = l.night_threshold {
            m.night_threshold = v;
        }
        if let Some(v) = l.night_hysteresis {
            m.night_hysteresis = v;
        }
        if let Some(v) = l.dark_mode_source {
            m.dark_mode_source = v;
        }
        if let Some(v) = l.night_dwell_s {
            m.night_dwell_s = v;
        }
        if l.clear_location {
            (m.latitude, m.longitude, m.utc_offset_hours) = (None, None, None);
        }
        if l.latitude.is_some() {
            m.latitude = l.latitude;
        }
        if l.longitude.is_some() {
            m.longitude = l.longitude;
        }
        if l.utc_offset_hours.is_some() {
            m.utc_offset_hours = l.utc_offset_hours;
        }
        if let Some(v) = &l.night_from {
            m.night_from = v.clone();
        }
        if let Some(v) = &l.day_from {
            m.day_from = v.clone();
        }
        if l.reset_learned {
            let src = m.ambient_source;
            m.learned.retain(|p| p.source != src);
        }
    }
}

#[cfg(test)]
pub(crate) fn sample() -> Settings {
    use carpi_proto::{
        AmbientSource, CurveMode, CurvePoint, DarkModeSource, DisSettings, LearnedPoint,
        LightSettings,
    };
    Settings {
        tx_enabled: false,
        power_shutdown: true,
        light: LightSettings {
            auto_brightness: true,
            scroll_trim: true,
            ambient_source: AmbientSource::Cluster,
            curve_mode: CurveMode::Learned,
            manual_curve: vec![
                CurvePoint {
                    ambient: 0.0,
                    pct: 12,
                },
                CurvePoint {
                    ambient: 1.5,
                    pct: 50,
                },
                CurvePoint {
                    ambient: 2.5,
                    pct: 100,
                },
            ],
            learned: vec![LearnedPoint {
                source: AmbientSource::Cluster,
                ambient: 1.0,
                pct: 30,
            }],
            dark_mode_source: DarkModeSource::Headlights,
            night_threshold: 1.2,
            night_hysteresis: 0.2,
            night_dwell_s: 10.0,
            latitude: None,
            longitude: None,
            utc_offset_hours: None,
            night_from: "19:00".into(),
            day_from: "07:00".into(),
        },
        dis: DisSettings {
            enabled: true,
            readings: vec![ReadingSetting {
                key: "engine.coolant_c".into(),
                label: "COL".into(),
                decimals: 0,
                scale: 1.0,
            }],
            claim: carpi_proto::DisClaim::Always,
            default_page: carpi_proto::DisPage::Readings,
            pages: Vec::new(),
            claim_nav: true,
            release_nav: true,
            claim_phone: true,
            release_phone: true,
            claim_media: true,
            release_media: true,
            media_layout: carpi_proto::DisMediaLayout::Stack,
            cover_flash_s: 4.0,
            units: carpi_proto::DisUnits::Auto,
            clock: carpi_proto::DisClock::Auto,
            nav_style: carpi_proto::DisNavStyle::Fancy,
            nav_distance: true,
            nav_pocket: true,
            nav_eta: true,
            nav_time_left: true,
            nav_dest_distance: true,
            nav_lanes: carpi_proto::DisLanes::Flip,
            accel: vec!["0-60mph".into(), "1/4mi".into(), "speed".into()],
        },
        bindings_custom: false,
        bindings: vec![
            Binding {
                table: "dis_toggle".into(),
                key: "mode".into(),
                gesture: "long".into(),
                action: "toggle".into(),
            },
            Binding {
                table: "livi".into(),
                key: "wheel_up".into(),
                gesture: "short".into(),
                action: "up".into(),
            },
        ],
        portal_pin_set: false,
        logs: Default::default(),
        clock: carpi_proto::ClockSettings {
            set_from_car: true,
            timezone: "America/Denver".into(),
            timezones: vec![
                "America/Denver".into(),
                "America/New_York".into(),
                "Europe/Berlin".into(),
                "UTC".into(),
            ],
        },
        power: Default::default(),
        input: Default::default(),
        portal: Default::default(),
        update: Default::default(),
        avsd: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_has_actions() {
        for t in [
            "dis_toggle",
            "system",
            "call_ringing",
            "call_active",
            "dis",
            "car",
            "livi",
        ] {
            assert!(!actions(t).is_empty(), "{t}");
        }
        assert!(actions("nope").is_empty());
    }

    #[test]
    fn readings_fill_replace_and_empty_slots() {
        let s = sample();
        let list = reading_set(&s.dis.readings, 1, Some("engine.rpm"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].key, "engine.rpm");
        assert_eq!(list[1].label, "ENG");
        let list = reading_set(&list, 0, Some("power.battery_v"));
        assert_eq!(list[0].key, "power.battery_v");
        assert_eq!(list[0].decimals, 2);
        let list = reading_set(&list, 0, None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].key, "engine.rpm");
        assert_eq!(dis_label("x.oil_temp"), "OIL");
    }

    #[test]
    fn a_patch_applies_like_carpid() {
        let mut s = sample();
        let rows = rebound(&s.bindings, 1, "select");
        apply(
            &mut s,
            &SettingsPatch {
                tx_enabled: Some(true),
                bindings: Some(rows),
                portal_pin: Some(PinChange::Set { pin: "4711".into() }),
                light: Some(carpi_proto::LightPatch {
                    reset_learned: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert!(s.tx_enabled && s.portal_pin_set);
        assert_eq!(s.bindings[1].action, "select");
        assert!(s.light.learned.is_empty());
        apply(
            &mut s,
            &SettingsPatch {
                dis: Some(carpi_proto::DisPatch {
                    claim: Some(carpi_proto::DisClaim::Tab),
                    claim_phone: Some(false),
                    cover_flash_s: Some(0.0),
                    units: Some(carpi_proto::DisUnits::Imperial),
                    clock: Some(carpi_proto::DisClock::H12),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(s.dis.claim, carpi_proto::DisClaim::Tab);
        assert!(!s.dis.claim_phone && s.dis.claim_nav);
        assert_eq!(s.dis.cover_flash_s, 0.0);
        assert_eq!(s.dis.units, carpi_proto::DisUnits::Imperial);
        assert_eq!(s.dis.clock, carpi_proto::DisClock::H12);
    }

    #[test]
    fn readings_pages_come_from_the_old_page_and_name_in_turn() {
        let s = sample();
        let pages = dis_pages(&s.dis);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].name, "Readings");
        assert_eq!(pages[0].cells[0].key, "engine.coolant_c");
        assert_eq!(page_name_step(&pages, 0, 1).as_deref(), Some("Engine"));
        let mut two = pages.clone();
        two.push(DisPageSetting {
            name: "Engine".into(),
            cells: vec![],
        });
        two[0].name = "Custom 4".into();
        assert_eq!(page_name_step(&two, 0, 1).as_deref(), Some("Boost"));
        assert_eq!(free_name(&two), "Boost");
        assert_eq!(units_for("°C"), ["°C", "°F"]);
        assert!((convert(1.0, "bar", "psi").unwrap() - 14.5).abs() < 0.01);
        assert_eq!(list_step(&[1, 2, 5], 2, 1), 5);
        assert_eq!(list_next(&[1, 2, 5], 5), 1);
        let zones = zone_regions(&sample().clock.timezones);
        assert_eq!(zones[0].0, "America");
        assert_eq!(zones[0].1.len(), 2);
        assert_eq!(zones.last().unwrap().0, "Other");
    }

    #[test]
    fn cover_flash_steps_and_stops() {
        assert_eq!(flash_step(4.0, 1), 5.0);
        assert_eq!(flash_step(4.0, -1), 3.0);
        assert_eq!(flash_step(0.0, -1), 0.0);
        assert_eq!(flash_step(15.0, 1), 15.0);
        assert_eq!(flash_step(6.0, 1), 8.0);
        assert_eq!(flash_step(6.0, -1), 5.0);
        assert_eq!(flash_step(60.0, -1), 15.0);
    }
}
