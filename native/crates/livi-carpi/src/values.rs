// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpid's car values as LIVI telemetry fields.

use std::collections::HashMap;

use carpi_proto::{Reading, Value as CarValue};
use serde_json::{Map, Value, json};

/// What LIVI asks carpid for.
pub const PREFIXES: [&str; 14] = [
    "vehicle.speed_kmh",
    "vehicle.odometer_km",
    "engine.rpm",
    "engine.coolant_c",
    "power.battery_v",
    "fuel.",
    "brakes.",
    "trans.selector",
    "lights.",
    "body.",
    "ambient.outside_temp_c",
    "steering.angle_deg",
    "light.night",
    "light.dim_display_pct",
];

/// Tank of the front wheel drive TT 8J. `LIVI_CARPI_TANK_L` sets another.
const TANK_L: f64 = 55.0;

const DOORS: [(&str, &str); 6] = [
    ("body.door_fl_open", "frontLeft"),
    ("body.door_fr_open", "frontRight"),
    ("body.door_rl_open", "rearLeft"),
    ("body.door_rr_open", "rearRight"),
    ("body.hood_open", "hood"),
    ("body.trunk_open", "trunk"),
];

pub fn tank_l() -> f64 {
    std::env::var("LIVI_CARPI_TANK_L")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|l| l.is_finite() && *l > 0.0)
        .unwrap_or(TANK_L)
}

/// The last valid value of each name. A reading that turned invalid drops out.
#[derive(Default)]
pub struct Values {
    known: HashMap<String, CarValue>,
    sent: Map<String, Value>,
}

impl Values {
    pub fn take(&mut self, items: Vec<Reading>) {
        for r in items {
            match r.value {
                Some(v) => self.known.insert(r.name, v),
                None => self.known.remove(&r.name),
            };
        }
    }

    /// The fields that differ from the last patch, `null` for one that went away.
    pub fn patch(&mut self, tank_l: f64) -> Option<Value> {
        let now = telemetry(&self.known, tank_l);
        let mut patch = Map::new();
        for (key, value) in &now {
            if self.sent.get(key) != Some(value) {
                patch.insert(key.clone(), value.clone());
            }
        }
        for key in self.sent.keys() {
            if !now.contains_key(key) {
                patch.insert(key.clone(), Value::Null);
            }
        }
        self.sent = now;
        (!patch.is_empty()).then_some(Value::Object(patch))
    }

    /// Clears every field once carpid is gone, so nothing stale reaches the phone.
    pub fn forget(&mut self) -> Option<Value> {
        self.known.clear();
        let patch: Map<String, Value> = self
            .sent
            .keys()
            .filter(|k| *k != "nightMode" && *k != "darkMode")
            .map(|k| (k.clone(), Value::Null))
            .collect();
        self.sent.clear();
        (!patch.is_empty()).then_some(Value::Object(patch))
    }
}

fn num(known: &HashMap<String, CarValue>, name: &str) -> Option<f64> {
    match known.get(name)? {
        CarValue::Num(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

fn flag(known: &HashMap<String, CarValue>, name: &str) -> Option<bool> {
    match known.get(name)? {
        CarValue::Bool(b) => Some(*b),
        _ => None,
    }
}

/// The selector lever as LIVI's gear: P, R, N, D, S or M1 to M4. The lever, not the gear the
/// box is in.
fn gear(selector: f64) -> Option<&'static str> {
    Some(match selector as u32 {
        1 => "M1",
        2 => "M2",
        3 => "M3",
        4 => "M4",
        5 => "D",
        6 => "N",
        7 => "R",
        8 => "P",
        9 | 12 => "S",
        _ => return None,
    })
}

fn telemetry(known: &HashMap<String, CarValue>, tank_l: f64) -> Map<String, Value> {
    let mut out = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(v) = value {
            out.insert(key.to_string(), v);
        }
    };
    let n = |name| num(known, name).map(Value::from);
    let f = |name| flag(known, name).map(Value::from);

    put("speedKph", n("vehicle.speed_kmh"));
    put("rpm", n("engine.rpm"));
    put("odometerKm", n("vehicle.odometer_km"));
    put("coolantC", n("engine.coolant_c"));
    put("batteryV", n("power.battery_v"));
    put("ambientC", n("ambient.outside_temp_c"));
    put("steeringDeg", n("steering.angle_deg"));
    put("dimmerPct", n("light.dim_display_pct"));
    put(
        "fuelPct",
        num(known, "fuel.level_l").map(|l| Value::from((l / tank_l * 100.0).clamp(0.0, 100.0))),
    );
    let brakes = [
        flag(known, "brakes.handbrake"),
        flag(known, "brakes.epb_closed"),
    ];
    if brakes.iter().any(Option::is_some) {
        put("parkingBrake", Some(brakes.contains(&Some(true)).into()));
    }
    put(
        "gear",
        num(known, "trans.selector").and_then(gear).map(Value::from),
    );
    put("reverse", f("lights.reverse"));
    put("lights", f("lights.low_beam"));
    put("highBeam", f("lights.high_beam"));
    put("hazards", f("lights.hazard"));
    let turn = match (
        flag(known, "lights.turn_left"),
        flag(known, "lights.turn_right"),
        flag(known, "lights.hazard"),
    ) {
        (_, _, Some(true)) | (Some(true), Some(true), _) => Some("none"),
        (Some(true), _, _) => Some("left"),
        (_, Some(true), _) => Some("right"),
        (Some(false), Some(false), _) => Some("none"),
        _ => None,
    };
    put("turnSignal", turn.map(Value::from));
    let doors: Map<String, Value> = DOORS
        .iter()
        .filter_map(|(name, field)| Some((field.to_string(), flag(known, name)?.into())))
        .collect();
    if !doors.is_empty() {
        put("doors", Some(Value::Object(doors)));
    }
    if let Some(night) = flag(known, "light.night") {
        put("nightMode", Some(json!(night)));
        put("darkMode", Some(json!(night)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(name: &str, value: Option<CarValue>) -> Reading {
        Reading {
            name: name.into(),
            value,
            age_ms: 0,
        }
    }

    #[test]
    fn car_values_become_telemetry_fields() {
        let mut v = Values::default();
        v.take(vec![
            reading("vehicle.speed_kmh", Some(CarValue::Num(73.5))),
            reading("fuel.level_l", Some(CarValue::Num(27.5))),
            reading("brakes.handbrake", Some(CarValue::Bool(false))),
            reading("brakes.epb_closed", None),
            reading("trans.selector", Some(CarValue::Num(8.0))),
            reading("lights.turn_left", Some(CarValue::Bool(true))),
            reading("lights.turn_right", Some(CarValue::Bool(false))),
            reading("body.door_fl_open", Some(CarValue::Bool(true))),
            reading("body.hood_open", Some(CarValue::Bool(false))),
            reading("light.night", Some(CarValue::Bool(true))),
        ]);
        assert_eq!(
            v.patch(55.0),
            Some(json!({
                "speedKph": 73.5,
                "fuelPct": 50.0,
                "parkingBrake": false,
                "gear": "P",
                "turnSignal": "left",
                "doors": { "frontLeft": true, "hood": false },
                "nightMode": true,
                "darkMode": true,
            }))
        );
        assert_eq!(v.patch(55.0), None);

        v.take(vec![
            reading("vehicle.speed_kmh", None),
            reading("brakes.epb_closed", Some(CarValue::Bool(true))),
        ]);
        assert_eq!(
            v.patch(55.0),
            Some(json!({ "speedKph": null, "parkingBrake": true }))
        );
        let gone = v.forget().unwrap();
        assert_eq!(gone["gear"], Value::Null);
        assert!(gone.get("nightMode").is_none() && gone.get("darkMode").is_none());
    }

    #[test]
    fn hazards_are_no_turn() {
        let mut v = Values::default();
        v.take(vec![
            reading("lights.turn_left", Some(CarValue::Bool(true))),
            reading("lights.turn_right", Some(CarValue::Bool(true))),
            reading("lights.hazard", Some(CarValue::Bool(true))),
        ]);
        let patch = v.patch(55.0).unwrap();
        assert_eq!(
            (&patch["turnSignal"], &patch["hazards"]),
            (&json!("none"), &json!(true))
        );
    }
}
