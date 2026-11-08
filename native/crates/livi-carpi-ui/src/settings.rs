// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpid's settings as the pages edit them: what each binding table takes, short labels for the
//! cluster display, and a patch applied the way carpid applies it (for the fixture and tests).

use carpi_proto::{Binding, PinChange, ReadingSetting, Settings, SettingsPatch};

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
        if let Some(v) = d.auto_nav {
            m.auto_nav = v;
        }
        if let Some(v) = d.auto_phone {
            m.auto_phone = v;
        }
        if let Some(v) = d.auto_media {
            m.auto_media = v;
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
            auto_nav: true,
            auto_phone: true,
            auto_media: true,
            media_layout: carpi_proto::DisMediaLayout::Stack,
            cover_flash_s: 4.0,
            units: carpi_proto::DisUnits::Auto,
            clock: carpi_proto::DisClock::Auto,
        },
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
                    auto_phone: Some(false),
                    cover_flash_s: Some(0.0),
                    units: Some(carpi_proto::DisUnits::Imperial),
                    clock: Some(carpi_proto::DisClock::H12),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(s.dis.claim, carpi_proto::DisClaim::Tab);
        assert!(!s.dis.auto_phone && s.dis.auto_nav);
        assert_eq!(s.dis.cover_flash_s, 0.0);
        assert_eq!(s.dis.units, carpi_proto::DisUnits::Imperial);
        assert_eq!(s.dis.clock, carpi_proto::DisClock::H12);
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
