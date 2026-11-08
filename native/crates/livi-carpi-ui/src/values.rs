// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpid's readings as the pages show them: the text, its unit, its age and whether it's still
//! good.
//!
//! carpid says a value is invalid (`None`) when the car sent an error or init value, or the
//! value's own timeout ran out. On top of that a value goes stale here when nothing refreshed it
//! for [`STALE_AFTER`]: carpid sends a subscription update only on change, so the pages poll the
//! full set every second to keep ages true, and a gap longer than that means carpid or the link is
//! gone.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use carpi_proto::{Reading, Value};

use crate::defs::{Kind, describe, selector};

/// A value no update refreshed for this long greys out.
pub const STALE_AFTER: Duration = Duration::from_secs(3);

/// A reading and when it arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub value: Option<Value>,
    /// Age carpid gave.
    pub age_ms: u64,
    pub at: Instant,
}

impl Seen {
    pub fn age(&self, now: Instant) -> Duration {
        Duration::from_millis(self.age_ms) + now.saturating_duration_since(self.at)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Valid when it came, but old now.
    Stale,
    /// carpid says there is no value.
    Invalid,
    /// carpid never sent the name.
    Missing,
}

impl Freshness {
    /// Shown greyed out.
    pub fn dim(self) -> bool {
        self != Freshness::Fresh
    }
}

pub fn freshness(seen: Option<&Seen>, now: Instant) -> Freshness {
    match seen {
        None => Freshness::Missing,
        Some(s) if s.value.is_none() => Freshness::Invalid,
        Some(s) if s.age(now) > STALE_AFTER => Freshness::Stale,
        Some(_) => Freshness::Fresh,
    }
}

/// Every reading carpid sent, by name.
#[derive(Debug, Clone, Default)]
pub struct Store {
    seen: BTreeMap<String, Seen>,
}

impl Store {
    pub fn take(&mut self, items: Vec<Reading>, at: Instant) {
        for r in items {
            self.seen.insert(
                r.name,
                Seen {
                    value: r.value,
                    age_ms: r.age_ms,
                    at,
                },
            );
        }
    }

    pub fn get(&self, name: &str) -> Option<&Seen> {
        self.seen.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.seen.keys().map(String::as_str)
    }

    /// Names starting with `prefix`, sorted.
    pub fn under<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a str, &'a Seen)> {
        self.seen
            .range(prefix.to_string()..)
            .take_while(move |(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v))
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// One value ready to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub name: String,
    pub label: String,
    pub value: String,
    pub unit: String,
    pub age: String,
    pub freshness: Freshness,
}

/// Shown when there is no value.
pub const NONE: &str = "—";

pub fn show(name: &str, seen: Option<&Seen>, now: Instant) -> Shown {
    let d = describe(name);
    let freshness = freshness(seen, now);
    let value = seen
        .and_then(|s| s.value.as_ref())
        .map(|v| text(v, d.kind))
        .unwrap_or_else(|| NONE.to_string());
    let unit = match seen.and_then(|s| s.value.as_ref()) {
        Some(Value::Num(_)) if !matches!(d.kind, Kind::Selector) => d.unit,
        _ => "",
    };
    Shown {
        name: name.to_string(),
        label: d.label.into_owned(),
        value,
        unit: unit.to_string(),
        age: match freshness {
            Freshness::Missing => "no data".into(),
            _ => seen.map(|s| age(s.age(now))).unwrap_or_default(),
        },
        freshness,
    }
}

/// The value as text, without its unit.
pub fn text(v: &Value, kind: Kind) -> String {
    match (v, kind) {
        (Value::Num(n), Kind::Selector) => selector(*n),
        (Value::Num(n), Kind::Num(dp)) => number(*n, dp),
        (Value::Num(n), Kind::Flag(on, off)) => if *n != 0.0 { on } else { off }.to_string(),
        (Value::Bool(b), Kind::Flag(on, off)) => if *b { on } else { off }.to_string(),
        (Value::Bool(b), _) => if *b { "Yes" } else { "No" }.to_string(),
        (Value::Text(t), _) => t.clone(),
    }
}

/// `n` with `dp` decimals; never `-0`.
pub fn number(n: f64, dp: u8) -> String {
    if !n.is_finite() {
        return NONE.to_string();
    }
    let s = format!("{n:.*}", dp as usize);
    match s.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
        _ => s,
    }
}

/// How old a value is, short.
pub fn age(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 10_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else if ms < 60_000 {
        format!("{} s", ms / 1000)
    } else if ms < 3_600_000 {
        format!("{} min", ms / 60_000)
    } else {
        format!("{} h", ms / 3_600_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(value: Option<Value>, age_ms: u64, at: Instant) -> Seen {
        Seen { value, age_ms, at }
    }

    #[test]
    fn numbers_keep_their_decimals_and_drop_negative_zero() {
        assert_eq!(number(812.25, 0), "812");
        assert_eq!(number(12.346, 2), "12.35");
        assert_eq!(number(-0.04, 1), "0.0");
        assert_eq!(number(-0.0, 0), "0");
        assert_eq!(number(-3.5, 1), "-3.5");
        assert_eq!(number(f64::NAN, 1), NONE);
    }

    #[test]
    fn values_read_by_kind() {
        let t0 = Instant::now();
        let s = show(
            "engine.rpm",
            Some(&seen(Some(Value::Num(812.25)), 40, t0)),
            t0,
        );
        assert_eq!((s.value.as_str(), s.unit.as_str()), ("812", "rpm"));
        assert_eq!(s.label, "Engine speed");
        let s = show(
            "power.battery_v",
            Some(&seen(Some(Value::Num(12.4)), 0, t0)),
            t0,
        );
        assert_eq!((s.value.as_str(), s.unit.as_str()), ("12.40", "V"));
        let door = seen(Some(Value::Bool(true)), 0, t0);
        let s = show("body.door_fl_open", Some(&door), t0);
        assert_eq!((s.value.as_str(), s.unit.as_str()), ("Open", ""));
        let sel = seen(Some(Value::Num(5.0)), 0, t0);
        assert_eq!(show("trans.selector", Some(&sel), t0).value, "D");
        let clock = seen(Some(Value::Text("2026-11-07 10:00".into())), 0, t0);
        assert_eq!(
            show("clock.datetime", Some(&clock), t0).value,
            "2026-11-07 10:00"
        );
        let s = show(
            "engine.oil_pressure_bar",
            Some(&seen(Some(Value::Num(2.0)), 0, t0)),
            t0,
        );
        assert_eq!((s.value.as_str(), s.unit.as_str()), ("2.0", "bar"));
    }

    #[test]
    fn invalid_and_missing_show_a_dash() {
        let t0 = Instant::now();
        let s = show("engine.rpm", Some(&seen(None, 0, t0)), t0);
        assert_eq!((s.value.as_str(), s.unit.as_str()), (NONE, ""));
        assert_eq!(s.freshness, Freshness::Invalid);
        let s = show("engine.maf_gs", None, t0);
        assert_eq!((s.value.as_str(), s.age.as_str()), (NONE, "no data"));
        assert_eq!(s.freshness, Freshness::Missing);
        assert!(s.freshness.dim());
    }

    #[test]
    fn a_value_goes_stale_when_nothing_refreshes_it() {
        let t0 = Instant::now();
        let s = seen(Some(Value::Num(1.0)), 500, t0);
        assert_eq!(freshness(Some(&s), t0), Freshness::Fresh);
        assert_eq!(
            freshness(Some(&s), t0 + Duration::from_millis(2500)),
            Freshness::Fresh
        );
        assert_eq!(
            freshness(Some(&s), t0 + Duration::from_millis(2501)),
            Freshness::Stale
        );
        let old = seen(Some(Value::Num(1.0)), 4000, t0);
        assert_eq!(freshness(Some(&old), t0), Freshness::Stale);
        let shown = show("engine.rpm", Some(&old), t0);
        assert_eq!((shown.value.as_str(), shown.age.as_str()), ("1", "4.0 s"));
        assert!(shown.freshness.dim());
    }

    #[test]
    fn ages_are_short() {
        assert_eq!(age(Duration::from_millis(40)), "0.0 s");
        assert_eq!(age(Duration::from_millis(1250)), "1.2 s");
        assert_eq!(age(Duration::from_millis(12_400)), "12 s");
        assert_eq!(age(Duration::from_secs(185)), "3 min");
        assert_eq!(age(Duration::from_secs(7300)), "2 h");
    }

    #[test]
    fn the_store_keeps_the_newest_reading() {
        let t0 = Instant::now();
        let mut st = Store::default();
        let r = |name: &str, v: f64| Reading {
            name: name.into(),
            value: Some(Value::Num(v)),
            age_ms: 0,
        };
        st.take(vec![r("light.night", 0.0), r("light.panel_level", 3.0)], t0);
        st.take(vec![r("light.panel_level", 4.0)], t0);
        assert_eq!(
            st.get("light.panel_level").unwrap().value,
            Some(Value::Num(4.0))
        );
        let under: Vec<_> = st.under("light.").map(|(n, _)| n).collect();
        assert_eq!(under, ["light.night", "light.panel_level"]);
        assert_eq!(st.under("rnse.").count(), 0);
    }
}
