// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The light page's plot (PLAN.md 2.6): ambient on a log scale across, RNS-E panel % up.
//!
//! The curve comes from carpid's `light.curve`, compact JSON the light model writes:
//! `{"source","mode","trim","points":[[ambient,pct],...],"learned":[[ambient,pct],...]}`. Without
//! it the plot falls back to the settings' manual curve and learned points. Ambient is carpid's
//! estimate, `log10(1 + raw)`, so the axis is linear in it and the ticks read as raw sensor values.

use carpi_proto::{AmbientSource, CurveMode, CurvePoint, LightSettings};
use serde_json::Value;

/// The plot's drawing box: SVG path commands use 0..BOX both ways, y down.
pub const BOX: f64 = 1000.0;
/// Smallest ambient range across, so a dark start still shows a useful axis.
const MIN_TOP: f64 = 2.0;

/// `light.curve` as carpid sends it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Curve {
    pub source: String,
    pub mode: String,
    /// Scroll trim in panel %, already in `points`.
    pub trim: f64,
    pub points: Vec<(f64, f64)>,
    pub learned: Vec<(f64, f64)>,
}

fn pairs(v: Option<&Value>) -> Vec<(f64, f64)> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    Some((p.first()?.as_f64()?, p.get(1)?.as_f64()?))
                })
                .filter(|(a, p)| a.is_finite() && p.is_finite())
                .collect()
        })
        .unwrap_or_default()
}

/// `None` for anything that isn't the curve.
pub fn parse(json: &str) -> Option<Curve> {
    let v: Value = serde_json::from_str(json).ok()?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let points = pairs(v.get("points"));
    if points.is_empty() {
        return None;
    }
    Some(Curve {
        source: text("source"),
        mode: text("mode"),
        trim: v.get("trim").and_then(Value::as_f64).unwrap_or(0.0),
        points,
        learned: pairs(v.get("learned")),
    })
}

/// A point in the plot, both 0..1, y up.
pub type At = (f32, f32);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plot {
    /// The curve as SVG path commands in a [`BOX`] square, empty without one.
    pub curve: String,
    pub learned: Vec<At>,
    /// The manual curve's points, while they are being edited.
    pub manual: Vec<At>,
    /// Current ambient and panel level.
    pub now: Option<At>,
    /// Across: where each tick goes and its raw sensor value.
    pub ticks: Vec<(f32, String)>,
    /// Under the plot: where the curve came from.
    pub caption: String,
}

pub fn source_name(s: AmbientSource) -> &'static str {
    match s {
        AmbientSource::Cluster => "cluster",
        AmbientSource::Climatronic => "climatronic",
        AmbientSource::Blend => "blend",
    }
}

pub fn next_source(s: AmbientSource) -> AmbientSource {
    match s {
        AmbientSource::Cluster => AmbientSource::Climatronic,
        AmbientSource::Climatronic => AmbientSource::Blend,
        AmbientSource::Blend => AmbientSource::Cluster,
    }
}

pub fn mode_name(m: CurveMode) -> &'static str {
    match m {
        CurveMode::Manual => "manual",
        CurveMode::Learned => "learned",
    }
}

/// An ambient estimate as the raw sensor value it stands for.
pub fn raw(ambient: f64) -> f64 {
    10f64.powf(ambient) - 1.0
}

/// Lays out the plot. `manual` adds the manual curve's points.
pub fn plot(
    curve: Option<&Curve>,
    settings: Option<&LightSettings>,
    ambient: Option<f64>,
    pct: Option<f64>,
    manual: bool,
) -> Plot {
    let fallback: Vec<(f64, f64)> = settings
        .map(|s| {
            s.manual_curve
                .iter()
                .map(|p| (p.ambient, f64::from(p.pct)))
                .collect()
        })
        .unwrap_or_default();
    let (line, learned, caption) = match curve {
        Some(c) => {
            let trim = if c.trim.abs() >= 0.5 {
                format!(", trim {:+.0} %", c.trim)
            } else {
                String::new()
            };
            (
                c.points.clone(),
                c.learned.clone(),
                format!("{} curve, {} sensor{trim}", c.mode, c.source),
            )
        }
        None => {
            let learned = settings
                .map(|s| {
                    s.learned
                        .iter()
                        .filter(|p| p.source == s.ambient_source)
                        .map(|p| (p.ambient, f64::from(p.pct)))
                        .collect()
                })
                .unwrap_or_default();
            let why = if fallback.is_empty() {
                "No curve from carpid yet"
            } else {
                "Manual curve from the settings; carpid sent no light.curve"
            };
            (fallback.clone(), learned, why.to_string())
        }
    };
    let manual_points = if manual { fallback } else { Vec::new() };
    let top = line
        .iter()
        .chain(&learned)
        .chain(&manual_points)
        .map(|p| p.0)
        .chain(ambient)
        .fold(MIN_TOP, f64::max);
    let top = (top * 2.0).ceil() / 2.0;
    let at = |(a, p): (f64, f64)| -> At {
        (
            (a / top).clamp(0.0, 1.0) as f32,
            (p / 100.0).clamp(0.0, 1.0) as f32,
        )
    };
    let curve = line
        .iter()
        .enumerate()
        .map(|(i, &(a, p))| {
            let x = (a / top).clamp(0.0, 1.0) * BOX;
            let y = BOX - (p / 100.0).clamp(0.0, 1.0) * BOX;
            format!("{} {x:.0} {y:.0}", if i == 0 { "M" } else { "L" })
        })
        .collect::<Vec<_>>()
        .join(" ");
    let ticks = [0.0, 10.0, 100.0, 1000.0, 10000.0]
        .into_iter()
        .map(|r: f64| ((r + 1.0).log10(), r))
        .filter(|(a, _)| *a <= top)
        .map(|(a, r)| ((a / top) as f32, format!("{r}")))
        .collect();
    Plot {
        curve,
        learned: learned.into_iter().map(at).collect(),
        manual: manual_points.into_iter().map(at).collect(),
        now: ambient.zip(pct).map(at),
        ticks,
        caption,
    }
}

/// Panel % step of one nudge or one press on a curve point.
pub const STEP_PCT: i16 = 5;
/// Ambient step of one press on a curve point.
pub const STEP_AMBIENT: f64 = 0.1;

/// The manual curve with point `at` moved by `pct` and `ambient`, kept between its neighbours.
pub fn moved(curve: &[CurvePoint], at: usize, pct: i16, ambient: f64) -> Vec<CurvePoint> {
    let mut out = curve.to_vec();
    let lo = if at == 0 { 0.0 } else { out[at - 1].ambient };
    let hi = out.get(at + 1).map_or(f64::MAX, |p| p.ambient);
    if let Some(p) = out.get_mut(at) {
        p.pct = (i16::from(p.pct) + pct).clamp(0, 100) as u8;
        p.ambient = ((p.ambient + ambient) * 100.0).round() / 100.0;
        p.ambient = p.ambient.clamp(lo, hi);
    }
    out
}

/// The manual curve with a point at `ambient`, in order.
pub fn added(curve: &[CurvePoint], ambient: f64, pct: u8) -> Vec<CurvePoint> {
    let mut out = curve.to_vec();
    let ambient = (ambient.max(0.0) * 100.0).round() / 100.0;
    let at = out.partition_point(|p| p.ambient < ambient);
    if out.get(at).is_some_and(|p| p.ambient == ambient) {
        out[at].pct = pct;
    } else {
        out.insert(
            at,
            CurvePoint {
                ambient,
                pct: pct.min(100),
            },
        );
    }
    out
}

pub fn removed(curve: &[CurvePoint], at: usize) -> Vec<CurvePoint> {
    let mut out = curve.to_vec();
    if at < out.len() {
        out.remove(at);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"source":"cluster","mode":"learned","trim":-3.0,"points":[[0.00,12],[1.20,40],[2.40,100]],"learned":[[0.60,18]]}"#;

    #[test]
    fn the_curve_reads_from_carpids_json() {
        let c = parse(JSON).unwrap();
        assert_eq!(c.points, [(0.0, 12.0), (1.2, 40.0), (2.4, 100.0)]);
        assert_eq!(c.learned, [(0.6, 18.0)]);
        assert_eq!((c.source.as_str(), c.mode.as_str()), ("cluster", "learned"));
        assert_eq!(parse("{}"), None);
        assert_eq!(parse("nope"), None);
    }

    #[test]
    fn the_plot_scales_ambient_and_percent() {
        let c = parse(JSON).unwrap();
        let p = plot(Some(&c), None, Some(1.25), Some(50.0), false);
        assert_eq!(p.curve, "M 0 880 L 480 600 L 960 0");
        assert_eq!(p.now, Some((0.5, 0.5)));
        assert_eq!(p.learned, [(0.24, 0.18)]);
        assert_eq!(
            p.ticks.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(),
            ["0", "10", "100"]
        );
        assert!(p.caption.contains("trim -3 %"), "{}", p.caption);
        let p = plot(None, None, None, Some(40.0), false);
        assert!(p.curve.is_empty() && p.now.is_none());
        assert_eq!(p.caption, "No curve from carpid yet");
    }

    #[test]
    fn manual_points_move_between_their_neighbours() {
        let c = vec![
            CurvePoint {
                ambient: 0.0,
                pct: 12,
            },
            CurvePoint {
                ambient: 1.0,
                pct: 98,
            },
            CurvePoint {
                ambient: 2.0,
                pct: 100,
            },
        ];
        let m = moved(&c, 1, STEP_PCT, 5.0);
        assert_eq!((m[1].ambient, m[1].pct), (2.0, 100));
        let m = moved(&c, 0, -20, -1.0);
        assert_eq!((m[0].ambient, m[0].pct), (0.0, 0));
        let a = added(&c, 1.456, 60);
        assert_eq!(a.len(), 4);
        assert_eq!((a[2].ambient, a[2].pct), (1.46, 60));
        assert_eq!(added(&c, 1.0, 50)[1].pct, 50);
        assert_eq!(removed(&c, 0).len(), 2);
        assert!((raw(2.0) - 99.0).abs() < 1e-9);
    }
}
