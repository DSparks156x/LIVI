// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the pages know about carpid's values: a label, a unit and how to show each one.
//!
//! Names follow carpid's store: `carpi-signals/src/ican.rs` for ICAN, `carpi-diag/src/catalog.rs`
//! for diagnostics, `carpi-core` for the light model. A name carpid sends that isn't here still
//! shows, with a label and unit guessed from the name.

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Def {
    pub name: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
}

const fn num(name: &'static str, label: &'static str, unit: &'static str, dp: u8) -> Def {
    Def {
        name,
        label,
        unit,
        kind: Kind::Num(dp),
    }
}

const fn flag(name: &'static str, label: &'static str, on: &'static str, off: &'static str) -> Def {
    Def {
        name,
        label,
        unit: "",
        kind: Kind::Flag(on, off),
    }
}

const OPEN: (&str, &str) = ("Open", "Closed");
const ON: (&str, &str) = ("On", "Off");
const YES: (&str, &str) = ("Yes", "No");

pub const DEFS: &[Def] = &[
    // ICAN, carpi-signals.
    num("engine.rpm", "Engine speed", "rpm", 0),
    num("engine.coolant_c", "Coolant", "°C", 0),
    num("vehicle.speed_kmh", "Speed", "km/h", 0),
    num("vehicle.odometer_km", "Odometer", "km", 0),
    num("vehicle.front_axle_pulses", "Front axle pulses", "", 0),
    num("vehicle.pulses_per_rev", "Pulses per rev", "", 0),
    num(
        "vehicle.tyre_circumference_mm",
        "Tyre circumference",
        "mm",
        0,
    ),
    num("power.battery_v", "Battery", "V", 2),
    num("ambient.outside_temp_c", "Outside", "°C", 1),
    num("fuel.level_l", "Fuel", "l", 0),
    flag("fuel.low_warning", "Fuel low", YES.0, YES.1),
    Def {
        name: "trans.selector",
        label: "Selector",
        unit: "",
        kind: Kind::Selector,
    },
    num("steering.angle_deg", "Steering angle", "°", 1),
    num("steering.rate_dps", "Steering rate", "°/s", 0),
    flag("brakes.epb_closed", "Parking brake", "Closed", "Open"),
    flag("brakes.handbrake", "Handbrake", ON.0, ON.1),
    num("wheels.fl_pulses", "Wheel FL pulses", "", 0),
    num("wheels.fr_pulses", "Wheel FR pulses", "", 0),
    num("wheels.rl_pulses", "Wheel RL pulses", "", 0),
    num("wheels.rr_pulses", "Wheel RR pulses", "", 0),
    flag("wheels.fl_reverse", "Wheel FL reverse", YES.0, YES.1),
    flag("wheels.fr_reverse", "Wheel FR reverse", YES.0, YES.1),
    flag("wheels.rl_reverse", "Wheel RL reverse", YES.0, YES.1),
    flag("wheels.rr_reverse", "Wheel RR reverse", YES.0, YES.1),
    flag("body.door_fl_open", "Door FL", OPEN.0, OPEN.1),
    flag("body.door_fr_open", "Door FR", OPEN.0, OPEN.1),
    flag("body.door_rl_open", "Door RL", OPEN.0, OPEN.1),
    flag("body.door_rr_open", "Door RR", OPEN.0, OPEN.1),
    flag("body.hood_open", "Bonnet", OPEN.0, OPEN.1),
    flag("lights.turn_left", "Turn left", ON.0, ON.1),
    flag("lights.turn_right", "Turn right", ON.0, ON.1),
    flag("lights.hazard", "Hazards", ON.0, ON.1),
    flag("lights.reverse", "Reverse lights", ON.0, ON.1),
    flag("lights.side", "Side lights", ON.0, ON.1),
    flag("lights.low_beam", "Low beam", ON.0, ON.1),
    flag("lights.high_beam", "High beam", ON.0, ON.1),
    flag("lights.fog_front", "Front fog", ON.0, ON.1),
    flag("lights.fog_rear", "Rear fog", ON.0, ON.1),
    flag("lights.drl", "DRL", ON.0, ON.1),
    flag("ignition.kl_s", "Key in (S)", ON.0, ON.1),
    flag("ignition.kl15", "Ignition (15)", ON.0, ON.1),
    flag("ignition.kl_x", "Load (X)", ON.0, ON.1),
    flag("ignition.kl50", "Cranking (50)", ON.0, ON.1),
    flag("ignition.kl_p", "Parking light (P)", ON.0, ON.1),
    flag("ignition.zas.s", "ZAS S", ON.0, ON.1),
    flag("ignition.zas.kl15", "ZAS 15", ON.0, ON.1),
    flag("ignition.zas.x", "ZAS X", ON.0, ON.1),
    flag("ignition.zas.kl50", "ZAS 50", ON.0, ON.1),
    num("light.display_pct", "Display dimming", "%", 0),
    num("light.kl58t_pct", "Terminal 58t", "%", 0),
    num("light.cluster_display_pct", "Cluster dimming", "%", 0),
    num("light.cluster_sensor", "Cluster light sensor", "", 0),
    num("light.dim_display_pct", "Gateway dimming", "%", 0),
    num("light.dim_switch_pct", "Terminal 58s", "%", 0),
    num("light.dim_sensor", "Gateway light sensor", "", 0),
    flag("rnse.radio_on", "RNS-E on", YES.0, YES.1),
    num("rnse.mode", "RNS-E mode", "", 0),
    flag("rnse.on_tv", "RNS-E on TV", YES.0, YES.1),
    // Light model, carpi-core.
    flag("light.night", "Night", YES.0, YES.1),
    flag("light.lights_on", "Lights on", YES.0, YES.1),
    num("light.panel_level", "Panel level", "", 0),
    num("light.ambient", "Ambient", "", 2),
    num("light.panel_pct", "RNS-E panel", "%", 0),
    num("light.target_pct", "Panel target", "%", 0),
    num("light.trim_pct", "Scroll trim", "%", 0),
    // Diagnostics catalog. Nothing until carpid reads diag.
    num("engine.maf_gs", "Air mass", "g/s", 1),
    num("engine.throttle_pct", "Throttle", "%", 0),
    num("engine.ignition_timing_deg", "Ignition timing", "°", 1),
    num("engine.load_pct", "Load", "%", 0),
    num("engine.atmospheric_mbar", "Atmospheric", "mbar", 0),
    num("engine.boost.actual", "Boost", "mbar", 0),
    num("engine.boost.specified", "Boost specified", "mbar", 0),
    num("engine.n75_duty_pct", "N75 duty", "%", 0),
    num("engine.intake_temp_c", "Intake", "°C", 0),
    num("engine.oil_temp_c", "Oil", "°C", 0),
    num("engine.injection_ms", "Injection", "ms", 2),
    num("engine.fuel_rail.actual_bar", "Rail", "bar", 0),
    num("engine.fuel_rail.specified_bar", "Rail specified", "bar", 0),
    num("engine.pedal_pct", "Pedal", "%", 0),
    num("engine.lambda.actual", "Lambda", "", 3),
    num("engine.lambda.specified", "Lambda specified", "", 3),
    num("engine.terminal30_v", "ECU terminal 30", "V", 1),
    num("engine.timing_retard.cyl1_deg", "Retard cyl 1", "°", 1),
    num("engine.timing_retard.cyl2_deg", "Retard cyl 2", "°", 1),
    num("engine.timing_retard.cyl3_deg", "Retard cyl 3", "°", 1),
    num("engine.timing_retard.cyl4_deg", "Retard cyl 4", "°", 1),
    num("awd.oil_temp_c", "Haldex oil", "°C", 0),
    num("awd.plate_temp_c", "Haldex plates", "°C", 0),
    num("awd.supply_v", "Haldex supply", "V", 1),
    num("awd.oil_pressure_bar", "Haldex pressure", "bar", 1),
    num("awd.estimated_torque_nm", "Haldex torque (est.)", "Nm", 0),
    num("awd.valve_opening_pct", "Haldex valve", "%", 0),
    num("awd.valve_current", "Haldex valve current", "", 0),
];

pub fn find(name: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.name == name)
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
    let mut label = stem.replace('_', " ");
    if let Some(first) = label.get(..1) {
        label = first.to_uppercase() + &label[1..];
    }
    Described {
        label: Cow::Owned(label),
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

/// A preset group of the data page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub id: &'static str,
    pub label: &'static str,
    pub preset: &'static [&'static str],
}

/// Most values the grid shows at once.
pub const MAX_CELLS: usize = 16;

pub const GROUPS: &[Group] = &[
    Group {
        id: "engine",
        label: "Engine",
        preset: &[
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
    Group {
        id: "drivetrain",
        label: "Drivetrain",
        preset: &[
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
    Group {
        id: "body",
        label: "Body/Lights",
        preset: &[
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
    Group {
        id: "electrical",
        label: "Electrical",
        preset: &[
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

/// Section of the value picker for a name: its first part.
pub fn section(name: &str) -> &'static str {
    match name.split('.').next().unwrap_or("") {
        "engine" => "Engine",
        "vehicle" | "wheels" | "steering" | "brakes" | "trans" => "Chassis",
        "body" => "Body",
        "lights" => "Lights",
        "light" => "Light model",
        "ignition" | "power" => "Electrical",
        "fuel" | "ambient" | "clock" => "Other",
        "rnse" => "RNS-E",
        "awd" => "Haldex",
        _ => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_known_and_fit_the_grid() {
        for g in GROUPS {
            assert!(g.preset.len() <= MAX_CELLS, "{}", g.id);
            for n in g.preset {
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
    fn unknown_names_get_a_guess() {
        let d = describe("engine.oil_pressure_bar");
        assert_eq!(
            (&*d.label, d.unit, d.kind),
            ("Oil pressure", "bar", Kind::Num(1))
        );
        let d = describe("haldex.mode");
        assert_eq!((&*d.label, d.unit), ("Mode", ""));
        assert_eq!(describe("engine.rpm").label, "Engine speed");
    }

    #[test]
    fn selector_positions() {
        assert_eq!(selector(5.0), "D");
        assert_eq!(selector(3.0), "3");
        assert_eq!(selector(8.0), "P");
        assert_eq!(selector(15.0), "? 15");
    }
}
