// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The catalog: what the pages know about carpid's values. A label, a unit, how each one reads,
//! and where it sits in the picker (category and sub-group, docs/PAGES.md 2).
//!
//! A plain table so it can move into carpid later. Names follow carpid's store:
//! `carpi-signals/src/ican.rs` for ICAN, `carpi-diag/src/catalog.rs` for diagnostics,
//! `carpi-core` for the light model, the bridge sections for phone and nav. Rows are in picker
//! order: actual before specified, FL FR RL RR, cylinders 1–4, everyday values first. A name
//! carpid sends that isn't here still shows, in Other, with a label and unit guessed from it.

use std::borrow::Cow;

/// How a value reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A number with this many decimals.
    Num(u8),
    /// A flag, shown as the first word when set and the second when not.
    Flag(&'static str, &'static str),
    /// `trans.selector`: the selector position code.
    Selector,
    /// Text as carpid sends it.
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Def {
    pub name: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
    /// Category id, see [`CATEGORIES`].
    pub cat: &'static str,
    /// Sub-group within the category.
    pub sub: &'static str,
}

/// A picker category and its sub-groups, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Category {
    pub id: &'static str,
    pub label: &'static str,
    pub subs: &'static [&'static str],
}

pub const OTHER: &str = "other";
/// Bridge values: phone and nav. The DIS and data logs take them, the Data page doesn't.
pub const PHONE: &str = "phone";

pub const CATEGORIES: &[Category] = &[
    Category {
        id: "engine",
        label: "Engine",
        subs: &["Basics", "Boost", "Fuel", "Timing"],
    },
    Category {
        id: "driving",
        label: "Driving",
        subs: &["Speed", "Gearbox", "Steering", "Brakes", "Wheels"],
    },
    Category {
        id: "haldex",
        label: "Haldex",
        subs: &["Temps", "Control", "Supply"],
    },
    Category {
        id: "trip",
        label: "Fuel & trip",
        subs: &["Fuel", "Outside", "Trip", "Clock"],
    },
    Category {
        id: "electrical",
        label: "Electrical",
        subs: &["Battery", "Ignition", "ZAS"],
    },
    Category {
        id: "body",
        label: "Body",
        subs: &["Doors"],
    },
    Category {
        id: "lights",
        label: "Lights",
        subs: &["Exterior", "Dimming", "Light model"],
    },
    Category {
        id: "rnse",
        label: "RNS-E",
        subs: &["RNS-E"],
    },
    Category {
        id: PHONE,
        label: "Phone & nav",
        subs: &["Nav", "Media", "Phone", "More"],
    },
    Category {
        id: OTHER,
        label: "Other",
        subs: &[],
    },
];

pub fn category(id: &str) -> Option<&'static Category> {
    CATEGORIES.iter().find(|c| c.id == id)
}

type At = (&'static str, &'static str);

const fn num(at: At, name: &'static str, label: &'static str, unit: &'static str, dp: u8) -> Def {
    Def {
        name,
        label,
        unit,
        kind: Kind::Num(dp),
        cat: at.0,
        sub: at.1,
    }
}

const fn flag(
    at: At,
    name: &'static str,
    label: &'static str,
    words: (&'static str, &'static str),
) -> Def {
    Def {
        name,
        label,
        unit: "",
        kind: Kind::Flag(words.0, words.1),
        cat: at.0,
        sub: at.1,
    }
}

const fn text(at: At, name: &'static str, label: &'static str) -> Def {
    Def {
        name,
        label,
        unit: "",
        kind: Kind::Text,
        cat: at.0,
        sub: at.1,
    }
}

const OPEN: (&str, &str) = ("Open", "Closed");
const ON: (&str, &str) = ("On", "Off");
const YES: (&str, &str) = ("Yes", "No");

const E_BASICS: At = ("engine", "Basics");
const E_BOOST: At = ("engine", "Boost");
const E_FUEL: At = ("engine", "Fuel");
const E_TIMING: At = ("engine", "Timing");
const D_SPEED: At = ("driving", "Speed");
const D_GEAR: At = ("driving", "Gearbox");
const D_STEER: At = ("driving", "Steering");
const D_BRAKES: At = ("driving", "Brakes");
const D_WHEELS: At = ("driving", "Wheels");
const H_TEMPS: At = ("haldex", "Temps");
const H_CONTROL: At = ("haldex", "Control");
const H_SUPPLY: At = ("haldex", "Supply");
const T_FUEL: At = ("trip", "Fuel");
const T_OUT: At = ("trip", "Outside");
const T_CLOCK: At = ("trip", "Clock");
const EL_BATT: At = ("electrical", "Battery");
const EL_IGN: At = ("electrical", "Ignition");
const EL_ZAS: At = ("electrical", "ZAS");
const B_DOORS: At = ("body", "Doors");
const L_EXT: At = ("lights", "Exterior");
const L_DIM: At = ("lights", "Dimming");
const L_MODEL: At = ("lights", "Light model");
const RNSE: At = ("rnse", "RNS-E");
const P_NAV: At = (PHONE, "Nav");
const P_MEDIA: At = (PHONE, "Media");
const P_PHONE: At = (PHONE, "Phone");

pub const DEFS: &[Def] = &[
    // Engine.
    num(E_BASICS, "engine.rpm", "Engine speed", "rpm", 0),
    num(E_BASICS, "engine.load_pct", "Load", "%", 0),
    num(E_BASICS, "engine.throttle_pct", "Throttle", "%", 0),
    num(E_BASICS, "engine.pedal_pct", "Pedal", "%", 0),
    num(E_BASICS, "engine.coolant_c", "Coolant", "°C", 0),
    num(E_BASICS, "engine.oil_temp_c", "Oil temp", "°C", 0),
    num(E_BASICS, "engine.intake_temp_c", "Intake temp", "°C", 0),
    num(E_BOOST, "engine.boost.actual", "Boost", "mbar", 0),
    num(
        E_BOOST,
        "engine.boost.specified",
        "Boost specified",
        "mbar",
        0,
    ),
    num(E_BOOST, "engine.n75_duty_pct", "N75 duty", "%", 0),
    num(E_BOOST, "engine.atmospheric_mbar", "Atmospheric", "mbar", 0),
    num(E_BOOST, "engine.maf_gs", "Air mass", "g/s", 1),
    num(
        E_FUEL,
        "engine.fuel_rail.actual_bar",
        "Rail pressure",
        "bar",
        0,
    ),
    num(
        E_FUEL,
        "engine.fuel_rail.specified_bar",
        "Rail specified",
        "bar",
        0,
    ),
    num(E_FUEL, "engine.injection_ms", "Injection", "ms", 2),
    num(E_FUEL, "engine.lambda.actual", "Lambda", "", 3),
    num(E_FUEL, "engine.lambda.specified", "Lambda specified", "", 3),
    num(
        E_TIMING,
        "engine.ignition_timing_deg",
        "Ignition timing",
        "°",
        1,
    ),
    num(
        E_TIMING,
        "engine.timing_retard.cyl1_deg",
        "Retard cyl 1",
        "°",
        1,
    ),
    num(
        E_TIMING,
        "engine.timing_retard.cyl2_deg",
        "Retard cyl 2",
        "°",
        1,
    ),
    num(
        E_TIMING,
        "engine.timing_retard.cyl3_deg",
        "Retard cyl 3",
        "°",
        1,
    ),
    num(
        E_TIMING,
        "engine.timing_retard.cyl4_deg",
        "Retard cyl 4",
        "°",
        1,
    ),
    // Driving.
    num(D_SPEED, "vehicle.speed_kmh", "Speed", "km/h", 0),
    num(D_SPEED, "vehicle.odometer_km", "Odometer", "km", 0),
    Def {
        name: "trans.selector",
        label: "Selector",
        unit: "",
        kind: Kind::Selector,
        cat: D_GEAR.0,
        sub: D_GEAR.1,
    },
    num(D_STEER, "steering.angle_deg", "Steering angle", "°", 1),
    num(D_STEER, "steering.rate_dps", "Steering rate", "°/s", 0),
    flag(D_BRAKES, "brakes.handbrake", "Handbrake", ON),
    flag(
        D_BRAKES,
        "brakes.epb_closed",
        "Parking brake",
        ("Closed", "Open"),
    ),
    num(D_WHEELS, "wheels.fl_pulses", "Pulses FL", "", 0),
    num(D_WHEELS, "wheels.fr_pulses", "Pulses FR", "", 0),
    num(D_WHEELS, "wheels.rl_pulses", "Pulses RL", "", 0),
    num(D_WHEELS, "wheels.rr_pulses", "Pulses RR", "", 0),
    flag(D_WHEELS, "wheels.fl_reverse", "Reverse FL", YES),
    flag(D_WHEELS, "wheels.fr_reverse", "Reverse FR", YES),
    flag(D_WHEELS, "wheels.rl_reverse", "Reverse RL", YES),
    flag(D_WHEELS, "wheels.rr_reverse", "Reverse RR", YES),
    num(D_WHEELS, "vehicle.front_axle_pulses", "Axle pulses", "", 0),
    num(D_WHEELS, "vehicle.pulses_per_rev", "Pulses per rev", "", 0),
    num(
        D_WHEELS,
        "vehicle.tyre_circumference_mm",
        "Tyre circumference",
        "mm",
        0,
    ),
    // Haldex.
    num(H_TEMPS, "awd.oil_temp_c", "Haldex oil", "°C", 0),
    num(H_TEMPS, "awd.plate_temp_c", "Haldex plates", "°C", 0),
    num(
        H_CONTROL,
        "awd.oil_pressure_bar",
        "Haldex pressure",
        "bar",
        1,
    ),
    num(
        H_CONTROL,
        "awd.estimated_torque_nm",
        "Haldex torque (est.)",
        "Nm",
        0,
    ),
    num(H_CONTROL, "awd.valve_opening_pct", "Haldex valve", "%", 0),
    num(
        H_CONTROL,
        "awd.valve_current",
        "Haldex valve current",
        "",
        0,
    ),
    num(H_SUPPLY, "awd.supply_v", "Haldex supply", "V", 1),
    // Fuel & trip.
    num(T_FUEL, "fuel.level_l", "Fuel", "l", 0),
    flag(T_FUEL, "fuel.low_warning", "Fuel low", YES),
    num(T_OUT, "ambient.outside_temp_c", "Outside", "°C", 1),
    text(T_CLOCK, "clock.datetime", "Car clock"),
    // Electrical.
    num(EL_BATT, "power.battery_v", "Battery", "V", 2),
    num(EL_BATT, "engine.terminal30_v", "ECU terminal 30", "V", 1),
    flag(EL_IGN, "ignition.kl_s", "Key in (S)", ON),
    flag(EL_IGN, "ignition.kl15", "Ignition (15)", ON),
    flag(EL_IGN, "ignition.kl_x", "Load (X)", ON),
    flag(EL_IGN, "ignition.kl50", "Cranking (50)", ON),
    flag(EL_IGN, "ignition.kl_p", "Parking light (P)", ON),
    flag(EL_ZAS, "ignition.zas.s", "ZAS S", ON),
    flag(EL_ZAS, "ignition.zas.kl15", "ZAS 15", ON),
    flag(EL_ZAS, "ignition.zas.x", "ZAS X", ON),
    flag(EL_ZAS, "ignition.zas.kl50", "ZAS 50", ON),
    // Body.
    flag(B_DOORS, "body.door_fl_open", "Door FL", OPEN),
    flag(B_DOORS, "body.door_fr_open", "Door FR", OPEN),
    flag(B_DOORS, "body.door_rl_open", "Door RL", OPEN),
    flag(B_DOORS, "body.door_rr_open", "Door RR", OPEN),
    flag(B_DOORS, "body.hood_open", "Bonnet", OPEN),
    // Lights.
    flag(L_EXT, "lights.side", "Side lights", ON),
    flag(L_EXT, "lights.low_beam", "Low beam", ON),
    flag(L_EXT, "lights.high_beam", "High beam", ON),
    flag(L_EXT, "lights.drl", "DRL", ON),
    flag(L_EXT, "lights.fog_front", "Front fog", ON),
    flag(L_EXT, "lights.fog_rear", "Rear fog", ON),
    flag(L_EXT, "lights.turn_left", "Turn left", ON),
    flag(L_EXT, "lights.turn_right", "Turn right", ON),
    flag(L_EXT, "lights.hazard", "Hazards", ON),
    flag(L_EXT, "lights.reverse", "Reverse lights", ON),
    num(L_DIM, "light.display_pct", "Display dimming", "%", 0),
    num(L_DIM, "light.kl58t_pct", "Terminal 58t", "%", 0),
    num(L_DIM, "light.dim_switch_pct", "Terminal 58s", "%", 0),
    num(
        L_DIM,
        "light.cluster_display_pct",
        "Cluster dimming",
        "%",
        0,
    ),
    num(L_DIM, "light.dim_display_pct", "Gateway dimming", "%", 0),
    num(L_DIM, "light.cluster_sensor", "Cluster light sensor", "", 0),
    num(L_DIM, "light.dim_sensor", "Gateway light sensor", "", 0),
    num(L_MODEL, "light.ambient", "Ambient", "", 2),
    flag(L_MODEL, "light.night", "Night", YES),
    num(L_MODEL, "light.panel_pct", "RNS-E panel", "%", 0),
    num(L_MODEL, "light.target_pct", "Panel target", "%", 0),
    num(L_MODEL, "light.trim_pct", "Scroll trim", "%", 0),
    flag(L_MODEL, "light.lights_on", "Lights on", YES),
    num(L_MODEL, "light.panel_level", "Panel level", "", 0),
    text(L_MODEL, "light.night_source", "Night by"),
    // RNS-E.
    flag(RNSE, "rnse.radio_on", "RNS-E on", YES),
    num(RNSE, "rnse.mode", "RNS-E mode", "", 0),
    flag(RNSE, "rnse.on_tv", "RNS-E on TV", YES),
    // Phone & nav, from the bridge.
    text(P_NAV, "nav.distance_text", "Next turn"),
    text(P_NAV, "nav.eta", "ETA"),
    text(P_NAV, "nav.road", "Road"),
    text(P_NAV, "nav.destination_distance_text", "To destination"),
    text(P_NAV, "nav.destination", "Destination"),
    text(P_MEDIA, "media.title", "Track"),
    text(P_MEDIA, "media.artist", "Artist"),
    text(P_MEDIA, "media.album", "Album"),
    text(P_PHONE, "call.state", "Call"),
    text(P_PHONE, "call.name", "Caller"),
    num(P_PHONE, "phone.battery_pct", "Phone battery", "%", 0),
    num(P_PHONE, "phone.signal_pct", "Phone signal", "%", 0),
];

pub fn find(name: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.name == name)
}

/// Where the bridge's sections land in carpid's store.
pub const BRIDGE: [&str; 6] = ["projection.", "nav.", "media.", "call.", "phone.", "ui."];

/// Store names no picker offers: the light model's curve is JSON for the plot, base64 is an
/// image, and the projection and focus are the bridge's own business.
pub fn hidden(name: &str) -> bool {
    name == "light.curve"
        || name.ends_with("_base64")
        || name.starts_with("projection.")
        || name.starts_with("ui.")
}

/// Category and sub-group of any name: the table's, or Other with the name's first part as the
/// sub-group. Unknown bridge names go with Phone & nav.
pub fn place(name: &str) -> (&'static str, Cow<'static, str>) {
    if let Some(d) = find(name) {
        return (d.cat, Cow::Borrowed(d.sub));
    }
    if BRIDGE.iter().any(|b| name.starts_with(b)) {
        return (PHONE, Cow::Borrowed("More"));
    }
    let prefix = name.split('.').next().unwrap_or(name);
    (OTHER, Cow::Owned(capital(prefix)))
}

fn capital(s: &str) -> String {
    let s = s.replace('_', " ");
    match s.get(..1) {
        Some(first) => first.to_uppercase() + &s[1..],
        None => s,
    }
}

/// Unit suffixes of carpid's names, longest first.
const SUFFIXES: &[(&str, &str, u8)] = &[
    ("_mbar", "mbar", 0),
    ("_kmh", "km/h", 0),
    ("_pct", "%", 0),
    ("_deg", "°", 1),
    ("_dps", "°/s", 0),
    ("_bar", "bar", 1),
    ("_km", "km", 0),
    ("_mm", "mm", 0),
    ("_ms", "ms", 1),
    ("_gs", "g/s", 1),
    ("_nm", "Nm", 0),
    ("_c", "°C", 1),
    ("_v", "V", 1),
    ("_l", "l", 0),
];

/// How a value shows: its label, unit and kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    pub label: Cow<'static, str>,
    pub unit: &'static str,
    pub kind: Kind,
}

/// The definition of `name`, or one guessed from it: the last part, unit suffix dropped, as the
/// label.
pub fn describe(name: &str) -> Described {
    if let Some(d) = find(name) {
        return Described {
            label: Cow::Borrowed(d.label),
            unit: d.unit,
            kind: d.kind,
        };
    }
    let last = name.rsplit('.').next().unwrap_or(name);
    let (stem, unit, dp) = SUFFIXES
        .iter()
        .find_map(|(s, u, dp)| last.strip_suffix(s).map(|stem| (stem, *u, *dp)))
        .unwrap_or((last, "", 2));
    Described {
        label: Cow::Owned(capital(stem)),
        unit,
        kind: Kind::Num(dp),
    }
}

/// The selector position for `trans.selector`, per 0x359.
pub fn selector(code: f64) -> String {
    match code as i64 {
        0 => "–".into(),
        n @ 1..=4 => n.to_string(),
        5 => "D".into(),
        6 => "N".into(),
        7 => "R".into(),
        8 => "P".into(),
        9 => "Tip".into(),
        10 => "Z1".into(),
        11 => "Z2".into(),
        12 => "S".into(),
        13 => "L".into(),
        14 => "Gate".into(),
        n => format!("? {n}"),
    }
}

/// A starting page of the Data page: today's groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub values: &'static [&'static str],
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "engine",
        name: "Engine",
        values: &[
            "engine.rpm",
            "vehicle.speed_kmh",
            "engine.coolant_c",
            "ambient.outside_temp_c",
            "power.battery_v",
            "fuel.level_l",
            "fuel.low_warning",
            "vehicle.odometer_km",
        ],
    },
    Preset {
        id: "drivetrain",
        name: "Drivetrain",
        values: &[
            "trans.selector",
            "vehicle.speed_kmh",
            "engine.rpm",
            "steering.angle_deg",
            "steering.rate_dps",
            "brakes.epb_closed",
            "brakes.handbrake",
            "wheels.fl_pulses",
            "wheels.fr_pulses",
            "wheels.rl_pulses",
            "wheels.rr_pulses",
            "vehicle.tyre_circumference_mm",
        ],
    },
    Preset {
        id: "body",
        name: "Body",
        values: &[
            "body.door_fl_open",
            "body.door_fr_open",
            "body.door_rl_open",
            "body.door_rr_open",
            "body.hood_open",
            "lights.side",
            "lights.low_beam",
            "lights.high_beam",
            "lights.drl",
            "lights.fog_front",
            "lights.fog_rear",
            "lights.reverse",
            "lights.turn_left",
            "lights.turn_right",
            "lights.hazard",
        ],
    },
    Preset {
        id: "electrical",
        name: "Electrical",
        values: &[
            "power.battery_v",
            "ignition.kl_s",
            "ignition.kl15",
            "ignition.kl_x",
            "ignition.kl50",
            "ignition.kl_p",
            "light.kl58t_pct",
            "light.dim_switch_pct",
            "light.display_pct",
            "light.dim_display_pct",
            "rnse.radio_on",
            "rnse.on_tv",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_sits_in_a_known_sub_group_in_order() {
        let mut last: Option<(usize, usize)> = None;
        for d in DEFS {
            let c = CATEGORIES
                .iter()
                .position(|c| c.id == d.cat)
                .unwrap_or_else(|| panic!("{} has no category {}", d.name, d.cat));
            let s = CATEGORIES[c]
                .subs
                .iter()
                .position(|s| *s == d.sub)
                .unwrap_or_else(|| panic!("{} has no sub-group {}", d.name, d.sub));
            assert!(
                last.is_none_or(|l| l <= (c, s)),
                "{} is out of order",
                d.name
            );
            last = Some((c, s));
        }
    }

    #[test]
    fn presets_are_known() {
        for p in PRESETS {
            assert!(p.values.len() <= 16, "{}", p.name);
            for n in p.values {
                assert!(find(n).is_some(), "{n} has no definition");
            }
        }
    }

    #[test]
    fn names_are_unique() {
        for (i, d) in DEFS.iter().enumerate() {
            assert!(DEFS[i + 1..].iter().all(|e| e.name != d.name), "{}", d.name);
        }
    }

    #[test]
    fn unknown_names_get_a_guess_and_go_to_other() {
        let d = describe("engine.oil_pressure_bar");
        assert_eq!(
            (&*d.label, d.unit, d.kind),
            ("Oil pressure", "bar", Kind::Num(1))
        );
        assert_eq!(describe("haldex.mode").label, "Mode");
        assert_eq!(describe("engine.rpm").label, "Engine speed");
        assert_eq!(place("engine.rpm"), ("engine", Cow::Borrowed("Basics")));
        assert_eq!(place("trip.avg_kmh").0, OTHER);
        assert_eq!(place("trip.avg_kmh").1, "Trip");
        assert_eq!(place("nav.lane_count").0, PHONE);
        assert!(hidden("light.curve") && hidden("nav.icon_png_base64") && hidden("ui.focus"));
    }

    #[test]
    fn selector_positions() {
        assert_eq!(selector(5.0), "D");
        assert_eq!(selector(3.0), "3");
        assert_eq!(selector(8.0), "P");
        assert_eq!(selector(15.0), "? 15");
    }
}
