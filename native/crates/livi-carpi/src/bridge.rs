// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! LIVI's state as carpid's bridge sections. Each section goes out only when it changed.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use carpi_proto::{
    BridgeUpdate, CallState, Lane, LaneArrow, MEDIA_ART_MAX_BYTES, MediaArt, MediaState, NavState,
    PhoneState, Projection, media_art_id, media_art_mime,
};
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
    /// Hash of the cover's base64 text, so an unchanged cover costs no decode.
    art: Option<Option<u64>>,
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
    /// `ui_path` is the path livi-ui reports, `/carpi/<page>` for one of ours.
    pub fn updates(&mut self, state: &State, ui_path: &str) -> Vec<BridgeUpdate> {
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
        let art = state
            .now_playing
            .artwork
            .as_deref()
            .filter(|a| !a.is_empty());
        if changed(&mut self.art, art.map(|a| media_art_id(a.as_bytes()))).is_some() {
            out.push(BridgeUpdate::MediaArt {
                art: art.and_then(media_art),
            });
        }
        if let Some(call) = changed(&mut self.call, call(&state.calls)) {
            out.push(BridgeUpdate::Call { call });
        }
        if let Some(phone) = changed(&mut self.phone, phone(state)) {
            out.push(BridgeUpdate::Phone { phone });
        }
        if let Some(focus) = changed(&mut self.focus, focus(state.front.main, ui_path)) {
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

fn focus(front: Front, ui_path: &str) -> String {
    match front {
        Front::Projection => "projection".into(),
        _ => livi_carpi_ui::focus(ui_path).unwrap_or_else(|| "livi".into()),
    }
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

/// CarPlay's verdict on a lane.
fn lane_status(code: u32) -> Option<&'static str> {
    match code {
        0 => Some("not_good"),
        1 => Some("good"),
        2 => Some("preferred"),
        _ => None,
    }
}

fn lane(l: &livi_core_proto::state::Lane) -> Lane {
    let mut arrows: Vec<LaneArrow> = l
        .angles
        .iter()
        .map(|&a| LaneArrow {
            shape: shape(a).to_string(),
            highlighted: false,
        })
        .collect();
    // Each highlight marks the arrow it points along, or the nearest one.
    for h in l.highlight_angles.iter().chain(&l.highlight_angle) {
        let nearest = l
            .angles
            .iter()
            .enumerate()
            .min_by_key(|(_, a)| (**a - h).abs())
            .map(|(i, _)| i);
        if let Some(i) = nearest {
            arrows[i].highlighted = true;
        }
    }
    Lane {
        arrows,
        status: l.status.and_then(lane_status).map(str::to_string),
    }
}

fn lanes(n: &Navigation) -> Vec<Lane> {
    if n.lane_guidance_shown != Some(true) {
        return Vec::new();
    }
    n.lane_guidance
        .iter()
        .find(|g| Some(g.index) == n.lane_guidance_index)
        .map(|g| g.lanes.iter().map(lane).collect())
        .unwrap_or_default()
}

/// LIVI keeps distance units as CarPlay counts them, for both protocols.
fn unit(code: Option<u32>) -> Option<&'static str> {
    match code? {
        0 => Some("km"),
        1 => Some("mi"),
        2 => Some("m"),
        3 => Some("yd"),
        4 => Some("ft"),
        _ => None,
    }
}

/// The phone's display text, with the unit added where it sends the number alone.
fn shown(display: &Option<String>, unit: Option<&str>) -> Option<String> {
    let d = display
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())?;
    Some(match unit {
        Some(u) if !d.chars().any(char::is_alphabetic) => format!("{d} {u}"),
        _ => d.to_string(),
    })
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
    let distance_unit = unit(n.remain_distance_units);
    let destination_unit = unit(n.distance_to_destination_units);
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
        distance_text: shown(&n.remain_distance_display, distance_unit)
            .or_else(|| text(&n.maneuver_distance_text)),
        distance_unit: distance_unit.map(str::to_string),
        time_to_maneuver_s: n.time_to_maneuver,
        road: text(&n.road_name),
        cue: next.and_then(|m| text(&m.description)),
        destination: text(&n.destination_name),
        destination_distance_m: n.distance_to_destination,
        destination_distance_text: shown(&n.distance_to_destination_display, destination_unit)
            .or_else(|| text(&n.destination_distance_text)),
        destination_distance_unit: destination_unit.map(str::to_string),
        time_to_destination_s: n.time_to_destination,
        eta: text(&n.eta_text),
        eta_unix_s: n
            .eta
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| v as i64),
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

/// LIVI keeps the cover as base64 of the phone's bytes; they go on as they are.
fn media_art(base64: &str) -> Option<MediaArt> {
    let bytes = STANDARD.decode(base64).ok()?;
    if bytes.is_empty() || bytes.len() > MEDIA_ART_MAX_BYTES {
        return None;
    }
    Some(MediaArt {
        id: media_art_id(&bytes),
        mime: media_art_mime(&bytes).into(),
        data_base64: base64.to_string(),
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
        let first = sent.updates(&state, "/media");
        assert_eq!(first.len(), 8);
        assert!(first.contains(&BridgeUpdate::MediaArt { art: None }));
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
        assert!(sent.updates(&state, "/media").is_empty());

        state.telemetry.insert("speedKph".into(), 50.into());
        assert!(sent.updates(&state, "/media").is_empty());
        state.front.main = Front::Projection;
        state.navigation.active = Some(false);
        assert_eq!(
            sent.updates(&state, "/media"),
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
    fn our_pages_report_their_focus() {
        let mut sent = Sent::default();
        let state = State::default();
        sent.updates(&state, "/settings");
        assert_eq!(
            sent.updates(&state, "/carpi/diagnostics"),
            [BridgeUpdate::Focus {
                focus: "page:diagnostics".into()
            }]
        );
        assert!(sent.updates(&state, "/carpi/diagnostics").is_empty());
        let mut shown = State::default();
        shown.front.main = Front::Projection;
        assert_eq!(
            sent.updates(&shown, "/carpi/diagnostics"),
            [BridgeUpdate::Focus {
                focus: "projection".into()
            }]
        );
        assert_eq!(
            sent.updates(&state, "/media"),
            [BridgeUpdate::Focus {
                focus: "livi".into()
            }]
        );
    }

    fn arrow(shape: &str, highlighted: bool) -> LaneArrow {
        LaneArrow {
            shape: shape.into(),
            highlighted,
        }
    }

    #[test]
    fn android_auto_sends_times_phone_text_and_every_highlighted_arrow() {
        let mut n = guided().navigation;
        n.time_to_maneuver = Some(21.0);
        n.remain_distance_display = Some("250 m".into());
        n.remain_distance_units = Some(2);
        n.distance_to_destination = Some(12_000.0);
        n.distance_to_destination_display = Some("12".into());
        n.distance_to_destination_units = Some(0);
        n.time_to_destination = Some(900.0);
        n.eta_text = Some("21:58".into());
        n.lane_guidance[0].lanes[1].highlight_angles = vec![45, 180];
        let nav = nav(&n).unwrap();
        assert_eq!(nav.time_to_maneuver_s, Some(21.0));
        assert_eq!(nav.distance_text.as_deref(), Some("250 m"));
        assert_eq!(nav.distance_unit.as_deref(), Some("m"));
        assert_eq!(nav.destination_distance_m, Some(12_000.0));
        assert_eq!(nav.destination_distance_text.as_deref(), Some("12 km"));
        assert_eq!(nav.destination_distance_unit.as_deref(), Some("km"));
        assert_eq!(nav.time_to_destination_s, Some(900.0));
        assert_eq!(nav.eta.as_deref(), Some("21:58"));
        assert_eq!(nav.eta_unix_s, None);
        assert_eq!(
            nav.lanes[1].arrows,
            [arrow("slight_right", true), arrow("uturn_right", true)]
        );
        assert!(nav.lanes[1].highlighted());
    }

    #[test]
    fn carplay_sends_its_units_epoch_eta_and_lane_status() {
        let mut state = guided();
        state.sessions.active = Some(Protocol::Carplay);
        let n = &mut state.navigation;
        n.maneuver_type = Some(2);
        n.remain_distance_display = Some("0.2".into());
        n.remain_distance_units = Some(1);
        n.distance_to_destination = Some(8_000.0);
        n.distance_to_destination_display = Some("5".into());
        n.distance_to_destination_units = Some(1);
        n.time_to_destination = Some(600.0);
        n.eta = Some(1_762_600_000.0);
        let lanes = &mut n.lane_guidance[0].lanes;
        lanes[0].status = Some(2);
        lanes[0].highlight_angle = Some(-85);
        lanes[1].status = Some(0);
        lanes[1].highlight_angle = None;
        let nav = nav(&state.navigation).unwrap();
        assert_eq!(nav.maneuver.as_deref(), Some("turn"));
        assert_eq!(nav.roundabout_exit, None);
        assert_eq!(nav.distance_text.as_deref(), Some("0.2 mi"));
        assert_eq!(nav.distance_unit.as_deref(), Some("mi"));
        assert_eq!(nav.destination_distance_text.as_deref(), Some("5 mi"));
        assert_eq!(nav.time_to_destination_s, Some(600.0));
        assert_eq!(nav.time_to_maneuver_s, None);
        assert_eq!(nav.eta_unix_s, Some(1_762_600_000));
        assert_eq!(
            nav.lanes,
            [
                Lane {
                    arrows: vec![arrow("normal_left", true), arrow("straight", false)],
                    status: Some("preferred".into()),
                },
                Lane {
                    arrows: vec![arrow("slight_right", false), arrow("uturn_right", false)],
                    status: Some("not_good".into()),
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
                road: Some("Ring".into()),
                cue: Some("Third exit".into()),
                lanes: vec![
                    Lane {
                        arrows: vec![arrow("normal_left", false), arrow("straight", true)],
                        status: None,
                    },
                    Lane {
                        arrows: vec![arrow("slight_right", false), arrow("uturn_right", false)],
                        status: None,
                    },
                ],
                ..Default::default()
            }
        );
        assert_eq!(maneuver(49, None), ("slight_turn", Some("left")));
        assert_eq!(maneuver(4, Some(SIDE_LEFT)), ("u_turn", Some("left")));
        assert_eq!(shape(-180), "uturn_left");
    }

    #[test]
    fn a_cover_goes_once_and_clears() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10];
        let mut sent = Sent::default();
        let mut state = State::default();
        state.now_playing.title = Some("One".into());
        state.now_playing.artwork = Some(STANDARD.encode(jpeg));
        let first = sent.updates(&state, "/media");
        let art = MediaArt {
            id: media_art_id(&jpeg),
            mime: "image/jpeg".into(),
            data_base64: STANDARD.encode(jpeg),
        };
        assert!(first.contains(&BridgeUpdate::MediaArt {
            art: Some(art.clone())
        }));

        // Same cover, new position or track text: not sent again.
        state.now_playing.elapsed_ms = Some(5000.0);
        state.now_playing.title = Some("Two".into());
        let next = sent.updates(&state, "/media");
        assert!(
            !next
                .iter()
                .any(|u| matches!(u, BridgeUpdate::MediaArt { .. }))
        );
        state.now_playing.artwork = Some(STANDARD.encode(jpeg));
        assert!(sent.updates(&state, "/media").is_empty());

        // A new cover goes, with its own id.
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        state.now_playing.artwork = Some(STANDARD.encode(png));
        let [BridgeUpdate::MediaArt { art: Some(new) }] = &sent.updates(&state, "/media")[..]
        else {
            panic!("one MediaArt");
        };
        assert_eq!(new.mime, "image/png");
        assert_ne!(new.id, art.id);

        // Gone (session ended), then empty or too big: cleared once.
        state.now_playing.artwork = None;
        assert_eq!(
            sent.updates(&state, "/media"),
            [BridgeUpdate::MediaArt { art: None }]
        );
        state.now_playing.artwork = Some(String::new());
        assert!(sent.updates(&state, "/media").is_empty());
        state.now_playing.artwork = Some(STANDARD.encode(vec![0xFF; MEDIA_ART_MAX_BYTES + 1]));
        assert_eq!(
            sent.updates(&state, "/media"),
            [BridgeUpdate::MediaArt { art: None }]
        );
    }
}
