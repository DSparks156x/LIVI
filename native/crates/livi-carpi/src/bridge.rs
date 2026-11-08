// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! LIVI's state as carpid's bridge sections. Each section goes out only when it changed.

use carpi_proto::{BridgeUpdate, CallState, Lane, MediaState, NavState, PhoneState, Projection};
use livi_core_proto::state::{
    Call, CallStatus, DeviceStatus, Front, Navigation, NowPlaying, Protocol, State,
};

const ROUNDABOUT_EXIT_FIRST: u32 = 28;
const ROUNDABOUT_EXIT_LAST: u32 = 46;
/// LIVI keeps the turn side the way CarPlay counts it.
const SIDE_RIGHT: u32 = 0;
const SIDE_LEFT: u32 = 1;
/// Android Auto reports signal in bars.
const SIGNAL_BARS: f64 = 4.0;

/// What carpid got last, `None` before the first push of a section.
#[derive(Default)]
pub struct Sent {
    projection: Option<Option<Projection>>,
    nav: Option<Option<NavState>>,
    icon: Option<Option<String>>,
    media: Option<Option<MediaState>>,
    call: Option<Option<CallState>>,
    phone: Option<Option<PhoneState>>,
    focus: Option<String>,
}

fn changed<T: PartialEq + Clone>(slot: &mut Option<T>, now: T) -> Option<T> {
    if slot.as_ref() == Some(&now) {
        return None;
    }
    *slot = Some(now.clone());
    Some(now)
}

impl Sent {
    pub fn updates(&mut self, state: &State) -> Vec<BridgeUpdate> {
        let mut out = Vec::new();
        if let Some(projection) = changed(&mut self.projection, projection(state)) {
            out.push(BridgeUpdate::Projection { projection });
        }
        if let Some(nav) = changed(&mut self.nav, nav(&state.navigation)) {
            out.push(BridgeUpdate::Nav { nav });
        }
        let icon = state
            .navigation
            .image
            .clone()
            .filter(|i| !i.is_empty() && state.navigation.active == Some(true));
        if let Some(png_base64) = changed(&mut self.icon, icon) {
            out.push(BridgeUpdate::NavIcon { png_base64 });
        }
        if let Some(media) = changed(&mut self.media, media(&state.now_playing)) {
            out.push(BridgeUpdate::Media { media });
        }
        if let Some(call) = changed(&mut self.call, call(&state.calls)) {
            out.push(BridgeUpdate::Call { call });
        }
        if let Some(phone) = changed(&mut self.phone, phone(state)) {
            out.push(BridgeUpdate::Phone { phone });
        }
        if let Some(focus) = changed(&mut self.focus, focus(state.front.main)) {
            out.push(BridgeUpdate::Focus { focus });
        }
        out
    }
}

fn projection(state: &State) -> Option<Projection> {
    let kind = match state.sessions.active? {
        Protocol::Androidauto => "android_auto",
        Protocol::Carplay => "carplay",
    };
    let wireless = state
        .devices
        .iter()
        .find(|d| d.status == DeviceStatus::Active)
        .and_then(|d| d.last_transport.as_deref())
        != Some("usb");
    Some(Projection {
        kind: kind.into(),
        wireless,
    })
}

fn focus(front: Front) -> String {
    match front {
        Front::Projection => "projection",
        _ => "livi",
    }
    .into()
}

/// A maneuver as Android Auto names it, with its side.
fn maneuver(code: u32, side: Option<u32>) -> (&'static str, Option<&'static str>) {
    let turn_side = match side {
        Some(SIDE_LEFT) => Some("left"),
        Some(SIDE_RIGHT) => Some("right"),
        _ => None,
    };
    let (left, right) = (Some("left"), Some("right"));
    match code {
        1 | 20 => ("turn", left),
        2 | 21 => ("turn", right),
        3 => ("straight", None),
        4 | 18 | 26 => ("u_turn", turn_side),
        5 => ("name_change", None),
        6 => ("roundabout_enter", turn_side),
        7 => ("roundabout_exit", turn_side),
        8 => ("off_ramp", turn_side),
        9 => ("on_ramp", turn_side),
        10 | 12 | 27 => ("destination", None),
        11 => ("depart", None),
        13 => ("fork", left),
        14 => ("fork", right),
        15..=17 => ("ferry_boat", None),
        19 | ROUNDABOUT_EXIT_FIRST..=ROUNDABOUT_EXIT_LAST => {
            ("roundabout_enter_and_exit", turn_side)
        }
        22 => ("off_ramp", left),
        23 => ("off_ramp", right),
        24 => ("destination", left),
        25 => ("destination", right),
        47 => ("sharp_turn", left),
        48 => ("sharp_turn", right),
        49 => ("slight_turn", left),
        50 => ("slight_turn", right),
        51 => ("merge", None),
        52 => ("merge", left),
        53 => ("merge", right),
        _ => ("unknown", None),
    }
}

/// The lane arrow's angle as Android Auto's lane shape, 0 straight ahead, negative left.
fn shape(angle: i32) -> &'static str {
    match angle {
        -22..=22 => "straight",
        -67..=-23 => "slight_left",
        23..=67 => "slight_right",
        -112..=-68 => "normal_left",
        68..=112 => "normal_right",
        -157..=-113 => "sharp_left",
        113..=157 => "sharp_right",
        ..=-158 => "uturn_left",
        _ => "uturn_right",
    }
}

fn lanes(n: &Navigation) -> Vec<Lane> {
    if n.lane_guidance_shown != Some(true) {
        return Vec::new();
    }
    n.lane_guidance
        .iter()
        .find(|g| Some(g.index) == n.lane_guidance_index)
        .map(|g| {
            g.lanes
                .iter()
                .map(|l| Lane {
                    shapes: l.angles.iter().map(|&a| shape(a).to_string()).collect(),
                    highlighted: l.highlight_angle.is_some(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn nav(n: &Navigation) -> Option<NavState> {
    if n.active != Some(true) {
        return None;
    }
    let text = |s: &Option<String>| s.clone().filter(|s| !s.is_empty());
    let (name, side) = match n.maneuver_type {
        Some(code) => {
            let (name, side) = maneuver(code, n.turn_side);
            (Some(name.to_string()), side.map(str::to_string))
        }
        None => (None, None),
    };
    let roundabout = n
        .maneuver_type
        .filter(|t| (ROUNDABOUT_EXIT_FIRST..=ROUNDABOUT_EXIT_LAST).contains(t));
    let next = n
        .current_maneuvers
        .first()
        .and_then(|i| n.maneuvers.iter().find(|m| m.index == *i));
    Some(NavState {
        maneuver: name,
        side,
        roundabout_exit: roundabout.map(|t| t - ROUNDABOUT_EXIT_FIRST + 1),
        roundabout_angle: roundabout.and(n.turn_angle),
        distance_m: n.remain_distance,
        distance_text: text(&n.remain_distance_display).or_else(|| text(&n.maneuver_distance_text)),
        time_to_maneuver_s: None,
        road: text(&n.road_name),
        cue: next.and_then(|m| text(&m.description)),
        destination: text(&n.destination_name),
        eta: text(&n.eta_text),
        lanes: lanes(n),
    })
}

fn media(np: &NowPlaying) -> Option<MediaState> {
    if *np == NowPlaying::default() {
        return None;
    }
    let ms = |v: Option<f64>| v.filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as u64);
    Some(MediaState {
        source: np.app.clone(),
        title: np.title.clone(),
        artist: np.artist.clone(),
        album: np.album.clone(),
        playing: np.playing == Some(true),
        position_ms: ms(np.elapsed_ms),
        duration_ms: ms(np.duration_ms),
    })
}

/// The call that matters most: one ringing, then one going on, then one held.
fn call(calls: &[Call]) -> Option<CallState> {
    let rank = |c: &&Call| match c.status {
        CallStatus::Incoming => 0,
        CallStatus::Active | CallStatus::Muted | CallStatus::Conference => 1,
        CallStatus::Held => 2,
        CallStatus::Unknown => 3,
    };
    let c = calls
        .iter()
        .filter(|c| c.status != CallStatus::Unknown)
        .min_by_key(rank)?;
    let state = match c.status {
        CallStatus::Incoming => "ringing",
        CallStatus::Held => "held",
        _ => "active",
    };
    Some(CallState {
        state: state.into(),
        number: c.number.clone(),
        name: c.name.clone(),
    })
}

fn phone(state: &State) -> Option<PhoneState> {
    let d = state
        .devices
        .iter()
        .find(|d| d.status == DeviceStatus::Active)?;
    let pct = |v: Option<f64>, full: f64| {
        v.filter(|v| v.is_finite())
            .map(|v| (v / full * 100.0).round().clamp(0.0, 100.0) as u8)
    };
    let phone = PhoneState {
        battery_pct: pct(d.battery_level, 100.0),
        signal_pct: pct(d.signal_strength, SIGNAL_BARS),
    };
    (phone != PhoneState::default()).then_some(phone)
}

#[cfg(test)]
mod tests {
    use livi_core_proto::state::{DeviceView, Lane as LiviLane, LaneGuidance, Maneuver, Sessions};

    use super::*;

    fn guided() -> State {
        State {
            sessions: Sessions {
                active: Some(Protocol::Androidauto),
                ..Default::default()
            },
            devices: vec![DeviceView {
                status: DeviceStatus::Active,
                last_transport: Some("wifi".into()),
                battery_level: Some(81.0),
                signal_strength: Some(3.0),
                ..Default::default()
            }],
            navigation: Navigation {
                active: Some(true),
                maneuver_type: Some(30),
                turn_side: Some(SIDE_RIGHT),
                turn_angle: Some(270),
                remain_distance: Some(250.0),
                maneuver_distance_text: Some("250 m".into()),
                road_name: Some("Ring".into()),
                image: Some("iVBO".into()),
                current_maneuvers: vec![0],
                maneuvers: vec![Maneuver {
                    index: 0,
                    description: Some("Third exit".into()),
                    ..Default::default()
                }],
                lane_guidance_index: Some(0),
                lane_guidance_shown: Some(true),
                lane_guidance: vec![LaneGuidance {
                    index: 0,
                    lanes: vec![
                        LiviLane {
                            angles: vec![-90, 0],
                            highlight_angle: Some(0),
                            ..Default::default()
                        },
                        LiviLane {
                            angles: vec![45, 180],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            },
            calls: vec![
                Call {
                    status: CallStatus::Held,
                    name: Some("Bob".into()),
                    ..Default::default()
                },
                Call {
                    status: CallStatus::Incoming,
                    number: Some("+49170".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn the_first_round_sends_every_section_then_only_changes() {
        let mut sent = Sent::default();
        let mut state = guided();
        let first = sent.updates(&state);
        assert_eq!(first.len(), 7);
        assert!(first.contains(&BridgeUpdate::Projection {
            projection: Some(Projection {
                kind: "android_auto".into(),
                wireless: true,
            })
        }));
        assert!(first.contains(&BridgeUpdate::Call {
            call: Some(CallState {
                state: "ringing".into(),
                number: Some("+49170".into()),
                name: None,
            })
        }));
        assert!(first.contains(&BridgeUpdate::Phone {
            phone: Some(PhoneState {
                battery_pct: Some(81),
                signal_pct: Some(75),
            })
        }));
        assert!(first.contains(&BridgeUpdate::Focus {
            focus: "livi".into()
        }));
        assert!(sent.updates(&state).is_empty());

        state.telemetry.insert("speedKph".into(), 50.into());
        assert!(sent.updates(&state).is_empty());
        state.front.main = Front::Projection;
        state.navigation.active = Some(false);
        assert_eq!(
            sent.updates(&state),
            [
                BridgeUpdate::Nav { nav: None },
                BridgeUpdate::NavIcon { png_base64: None },
                BridgeUpdate::Focus {
                    focus: "projection".into()
                },
            ]
        );
    }

    #[test]
    fn guidance_reads_as_android_auto_names_it() {
        let nav = nav(&guided().navigation).unwrap();
        assert_eq!(
            nav,
            NavState {
                maneuver: Some("roundabout_enter_and_exit".into()),
                side: Some("right".into()),
                roundabout_exit: Some(3),
                roundabout_angle: Some(270),
                distance_m: Some(250.0),
                distance_text: Some("250 m".into()),
                time_to_maneuver_s: None,
                road: Some("Ring".into()),
                cue: Some("Third exit".into()),
                destination: None,
                eta: None,
                lanes: vec![
                    Lane {
                        shapes: vec!["normal_left".into(), "straight".into()],
                        highlighted: true,
                    },
                    Lane {
                        shapes: vec!["slight_right".into(), "uturn_right".into()],
                        highlighted: false,
                    },
                ],
            }
        );
        assert_eq!(maneuver(49, None), ("slight_turn", Some("left")));
        assert_eq!(maneuver(4, Some(SIDE_LEFT)), ("u_turn", Some("left")));
        assert_eq!(shape(-180), "uturn_left");
    }
}
