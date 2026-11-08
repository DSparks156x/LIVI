// SPDX-FileCopyrightText: 2025-2026 Lasse Heitgres
// SPDX-License-Identifier: GPL-3.0-or-later
// Additional terms: see NOTICE

use crate::channels::audio::AudioChannelType;
use crate::channels::media_info::{MediaPlaybackMetadata, MediaPlaybackState, MediaPlaybackStatus};
use crate::channels::nav_maneuver::{
    DrivingSide, ManeuverType, lane_shape_angle, nav_maneuver_type_to_code,
    nav_maneuver_type_to_side, roundabout_exit, turn_event_to_maneuver_type,
    turn_side_to_navi_code,
};
use crate::channels::navigation::{
    NavigationEvent, NavigationLane, NavigationState, NavigationStep,
};
use crate::config::AaConfig;
use crate::discovery::VideoCodec;
use crate::log::detail;
use crate::media::AudioKind;
use crate::protocol::{PhoneCall, SessionEvent};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NowPlaying {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub app: Option<String>,
    pub duration_ms: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub playing: Option<bool>,
    pub artwork: Option<Vec<u8>>,
}

impl NowPlaying {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Navigation {
    pub active: Option<bool>,
    pub app_name: Option<String>,
    pub road_name: Option<String>,
    pub maneuver_type: Option<ManeuverType>,
    pub turn_side: Option<DrivingSide>,
    pub turn_angle: Option<u32>,
    pub roundabout_exit_number: Option<u32>,
    pub remain_distance: Option<u32>,
    pub time_to_turn: Option<u32>,
    /// Thousandths of the distance in `display_distance_unit` (AA's distance unit code).
    /// The phone's display distance is this or `step_distance_display`, whichever came last.
    pub display_distance_e3: Option<u32>,
    pub display_distance_unit: Option<u32>,
    /// The distance to the step as the phone writes it ("300 m"), with its unit.
    pub step_distance_display: Option<String>,
    pub step_distance_unit: Option<u32>,
    pub destination_name: Option<String>,
    pub distance_to_destination: Option<u32>,
    pub destination_distance_display: Option<String>,
    pub destination_distance_unit: Option<u32>,
    pub time_to_destination: Option<u32>,
    pub eta: Option<String>,
    /// The next step first, replaced whole by every state the phone sends.
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    pub maneuver_type: Option<ManeuverType>,
    pub turn_side: Option<DrivingSide>,
    pub turn_angle: Option<u32>,
    pub road_name: Option<String>,
    pub cue: Option<String>,
    /// Left to right.
    pub lanes: Vec<Lane>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lane {
    /// One arrow per direction, see [`lane_shape_angle`].
    pub angles: Vec<i32>,
    /// The first arrow to take.
    pub highlighted: Option<i32>,
    /// Every arrow to take, in `angles` order.
    pub highlights: Vec<i32>,
}

impl Step {
    fn of(s: NavigationStep) -> Self {
        let maneuver_type = match nav_maneuver_type_to_code(s.maneuver_type) {
            Some(ManeuverType::EnterRoundabout) => s
                .roundabout_exit_number
                .and_then(roundabout_exit)
                .or(Some(ManeuverType::EnterRoundabout)),
            other => other,
        };
        Self {
            maneuver_type,
            turn_side: nav_maneuver_type_to_side(s.maneuver_type),
            turn_angle: s.roundabout_exit_angle,
            road_name: s.road_name.filter(|r| !r.is_empty()),
            cue: s.cue.filter(|c| !c.is_empty()),
            lanes: s.lanes.into_iter().map(Lane::of).collect(),
        }
    }
}

impl Lane {
    fn of(lane: NavigationLane) -> Self {
        let mut out = Self::default();
        for d in lane.directions {
            let Some(angle) = lane_shape_angle(d.shape) else {
                continue;
            };
            out.angles.push(angle);
            if d.highlighted {
                out.highlighted.get_or_insert(angle);
                out.highlights.push(angle);
            }
        }
        out
    }
}

impl Navigation {
    fn merge(&mut self, patch: Navigation) {
        fn take<T>(into: &mut Option<T>, from: Option<T>) {
            if from.is_some() {
                *into = from;
            }
        }
        take(&mut self.active, patch.active);
        take(&mut self.app_name, patch.app_name);
        take(&mut self.road_name, patch.road_name);
        take(&mut self.maneuver_type, patch.maneuver_type);
        take(&mut self.turn_side, patch.turn_side);
        take(&mut self.turn_angle, patch.turn_angle);
        take(
            &mut self.roundabout_exit_number,
            patch.roundabout_exit_number,
        );
        take(&mut self.remain_distance, patch.remain_distance);
        take(&mut self.time_to_turn, patch.time_to_turn);
        take(&mut self.display_distance_e3, patch.display_distance_e3);
        take(&mut self.display_distance_unit, patch.display_distance_unit);
        take(&mut self.step_distance_display, patch.step_distance_display);
        take(&mut self.step_distance_unit, patch.step_distance_unit);
        take(&mut self.destination_name, patch.destination_name);
        take(
            &mut self.distance_to_destination,
            patch.distance_to_destination,
        );
        take(
            &mut self.destination_distance_display,
            patch.destination_distance_display,
        );
        take(
            &mut self.destination_distance_unit,
            patch.destination_distance_unit,
        );
        take(&mut self.time_to_destination, patch.time_to_destination);
        take(&mut self.eta, patch.eta);
        if !patch.steps.is_empty() {
            self.steps = patch.steps;
        }
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhoneStatus {
    pub ip: String,
    pub battery_level: Option<u32>,
    pub battery_critical: Option<bool>,
    pub battery_time_remaining_s: Option<u32>,
    pub signal_strength: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    Connected,
    Disconnected,
    Device {
        name: String,
        model: String,
        instance_id: String,
        ip: String,
    },
    Status(PhoneStatus),
    Calls(Vec<PhoneCall>),
    VideoCodec {
        cluster: bool,
        codec: VideoCodec,
    },
    VideoFocus {
        cluster: bool,
        projected: bool,
    },
    HostUiRequested,
    Audio {
        stream: AudioChannelType,
        sample_rate: u32,
        channels: u32,
        active: bool,
    },
    Duck {
        level: f64,
        duration_ms: u32,
    },
    NowPlaying(NowPlaying),
    Navigation(Navigation),
    NavigationImage(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
// Navigation reports dominate; they're moved once per phone event, boxing buys nothing.
#[allow(clippy::large_enum_variant)]
pub enum BridgeOut {
    Report(Report),
    StartMic(&'static str),
    StopMic(&'static str),
    VideoSink {
        cluster: bool,
        codec: VideoCodec,
    },
    AudioSinks,
    PrimeAudio {
        kind: AudioKind,
        sample_rate: u32,
        channels: u32,
        tag: AudioChannelType,
    },
    VideoStarted {
        cluster: bool,
        width: u32,
        height: u32,
    },
}

const GOOGLE_MAPS: &str = "Google Maps";

#[derive(Debug, Default)]
pub struct Bridge {
    navi: Navigation,
    navi_active: bool,
    navi_app: Option<String>,
    video_focus: bool,
    cluster_focus: bool,
}

fn report(out: &mut Vec<BridgeOut>, r: Report) {
    out.push(BridgeOut::Report(r));
}

impl Bridge {
    pub fn reset(&mut self) -> Vec<BridgeOut> {
        let mut out = Vec::new();
        self.navi = Navigation::default();
        self.navi_active = false;
        self.navi_app = None;
        if self.video_focus {
            self.video_focus = false;
            report(
                &mut out,
                Report::VideoFocus {
                    cluster: false,
                    projected: false,
                },
            );
        }
        out
    }

    pub fn on_event(&mut self, event: SessionEvent, cfg: &AaConfig) -> Vec<BridgeOut> {
        let mut out = Vec::new();
        match event {
            SessionEvent::Connected => {
                println!("[AaEventBridge] session connected");
                report(&mut out, Report::Connected);
                out.push(BridgeOut::AudioSinks);
            }
            SessionEvent::Disconnected(reason) => {
                println!("[AaEventBridge] session disconnected ({reason})");
                out.extend(self.reset());
                report(&mut out, Report::Disconnected);
            }
            SessionEvent::Error(e) => println!("[AaEventBridge] session error: {e}"),
            SessionEvent::DeviceInfo {
                name,
                model,
                instance_id,
                ip,
            } => {
                report(
                    &mut out,
                    Report::Device {
                        name,
                        model,
                        instance_id,
                        ip,
                    },
                );
            }
            SessionEvent::Battery {
                ip,
                level,
                critical,
                time_remaining_s,
            } => {
                report(
                    &mut out,
                    Report::Status(PhoneStatus {
                        ip,
                        battery_level: level,
                        battery_critical: Some(critical),
                        battery_time_remaining_s: time_remaining_s,
                        signal_strength: None,
                    }),
                );
            }
            SessionEvent::Signal { ip, strength } => {
                report(
                    &mut out,
                    Report::Status(PhoneStatus {
                        ip,
                        signal_strength: Some(strength),
                        ..Default::default()
                    }),
                );
            }
            SessionEvent::Calls { calls, .. } => report(&mut out, Report::Calls(calls)),
            SessionEvent::VideoFocusProjected => {
                self.video_focus = true;
                report(
                    &mut out,
                    Report::VideoFocus {
                        cluster: false,
                        projected: true,
                    },
                );
            }
            SessionEvent::ClusterVideoFocusProjected => {
                self.cluster_focus = true;
                report(
                    &mut out,
                    Report::VideoFocus {
                        cluster: true,
                        projected: true,
                    },
                );
            }
            SessionEvent::VideoCodec(codec) | SessionEvent::ClusterVideoCodec(codec) => {
                let cluster = matches!(event, SessionEvent::ClusterVideoCodec(_));
                detail!(
                    "[AaEventBridge] {}video codec {} (phone selection)",
                    if cluster { "cluster " } else { "" },
                    codec.name()
                );
                report(&mut out, Report::VideoCodec { cluster, codec });
                out.push(BridgeOut::VideoSink { cluster, codec });
            }
            SessionEvent::VideoStarted => {
                if !self.video_focus {
                    self.video_focus = true;
                    report(
                        &mut out,
                        Report::VideoFocus {
                            cluster: false,
                            projected: true,
                        },
                    );
                }
                out.push(BridgeOut::VideoStarted {
                    cluster: false,
                    width: cfg.video_width.unwrap_or(1280),
                    height: cfg.video_height.unwrap_or(720),
                });
            }
            SessionEvent::ClusterVideoStarted => {
                if !self.cluster_focus {
                    self.cluster_focus = true;
                    report(
                        &mut out,
                        Report::VideoFocus {
                            cluster: true,
                            projected: true,
                        },
                    );
                }
                out.push(BridgeOut::VideoStarted {
                    cluster: true,
                    width: cfg.cluster_tier_width.unwrap_or(0),
                    height: cfg.cluster_tier_height.unwrap_or(0),
                });
            }
            SessionEvent::AudioSetup {
                channel,
                sample_rate,
                channels,
            } => {
                out.push(BridgeOut::PrimeAudio {
                    kind: AudioKind::of(channel),
                    sample_rate,
                    channels,
                    tag: channel,
                });
            }
            SessionEvent::Audio {
                channel,
                sample_rate,
                channels,
                active,
            } => {
                detail!(
                    "[AaEventBridge] audio {} {} ({sample_rate}Hz {channels}ch)",
                    channel.name(),
                    if active { "start" } else { "stop" }
                );
                report(
                    &mut out,
                    Report::Audio {
                        stream: channel,
                        sample_rate,
                        channels,
                        active,
                    },
                );
            }
            SessionEvent::AudioFocus(focus_type) => {
                let (level, duration_ms) = if focus_type == 3 {
                    (0.2, 500)
                } else {
                    (1.0, 1500)
                };
                detail!("[AaEventBridge] audio-focus type={focus_type} -> duck level={level}");
                report(&mut out, Report::Duck { level, duration_ms });
            }
            SessionEvent::MicStart => out.push(BridgeOut::StartMic("mic-start")),
            SessionEvent::MicStop => out.push(BridgeOut::StopMic("mic-stop")),
            SessionEvent::VoiceSession(true) => {
                out.push(BridgeOut::StartMic("voice-session START"))
            }
            SessionEvent::VoiceSession(false) => out.push(BridgeOut::StopMic("voice-session END")),
            SessionEvent::HostUiRequested => {
                println!("[AaEventBridge] host UI requested");
                report(&mut out, Report::HostUiRequested);
            }
            SessionEvent::MediaMetadata(m) => self.metadata(&mut out, m),
            SessionEvent::MediaStatus(s) => self.status(&mut out, s),
            SessionEvent::Navigation(n) => self.navigation(&mut out, n),
        }
        out
    }

    fn metadata(&mut self, out: &mut Vec<BridgeOut>, m: MediaPlaybackMetadata) {
        let now = NowPlaying {
            title: m.song,
            artist: m.artist,
            album: m.album,
            duration_ms: m.duration_seconds.map(|d| u64::from(d) * 1000),
            artwork: m.album_art.filter(|a| !a.is_empty()),
            ..Default::default()
        };
        if !now.is_empty() {
            report(out, Report::NowPlaying(now));
        }
    }

    fn status(&mut self, out: &mut Vec<BridgeOut>, s: MediaPlaybackStatus) {
        report(
            out,
            Report::NowPlaying(NowPlaying {
                playing: Some(s.state == MediaPlaybackState::Playing),
                app: s.media_source,
                elapsed_ms: s.playback_seconds.map(|p| u64::from(p) * 1000),
                ..Default::default()
            }),
        );
    }

    fn navigation(&mut self, out: &mut Vec<BridgeOut>, event: NavigationEvent) {
        match event {
            NavigationEvent::Start => {
                self.navi_app = Some(GOOGLE_MAPS.to_string());
                self.navi_active = true;
                self.publish(
                    out,
                    Navigation {
                        active: Some(true),
                        app_name: self.navi_app.clone(),
                        ..Default::default()
                    },
                );
            }
            NavigationEvent::Stop => {
                self.navi_active = false;
                self.publish(
                    out,
                    Navigation {
                        active: Some(false),
                        ..Default::default()
                    },
                );
                self.navi = Navigation::default();
            }
            NavigationEvent::Status(state) => {
                self.navi_active =
                    matches!(state, NavigationState::Active | NavigationState::Rerouting);
                let active = Some(self.navi_active);
                self.publish(
                    out,
                    Navigation {
                        active,
                        ..Default::default()
                    },
                );
            }
            NavigationEvent::Turn(t) => {
                let maneuver_type = match turn_event_to_maneuver_type(t.event, t.turn_side) {
                    Some(ManeuverType::EnterRoundabout) => t
                        .turn_number
                        .and_then(roundabout_exit)
                        .or(Some(ManeuverType::EnterRoundabout)),
                    other => other,
                };
                let patch = Navigation {
                    road_name: t.road.clone(),
                    maneuver_type,
                    turn_side: turn_side_to_navi_code(t.turn_side),
                    turn_angle: t.turn_angle,
                    roundabout_exit_number: t.turn_number,
                    ..Default::default()
                };
                if !patch.is_empty() {
                    self.publish(out, patch);
                }
                if let Some(image) = t.image.filter(|i| !i.is_empty()) {
                    report(out, Report::NavigationImage(image));
                }
            }
            NavigationEvent::Distance(d) => {
                if d.display_distance_e3.is_some() {
                    self.navi.step_distance_display = None;
                    self.navi.step_distance_unit = None;
                }
                self.publish(
                    out,
                    Navigation {
                        remain_distance: Some(d.distance_meters),
                        time_to_turn: Some(d.time_to_turn_seconds),
                        display_distance_e3: d.display_distance_e3,
                        display_distance_unit: d.display_unit,
                        ..Default::default()
                    },
                );
            }
            NavigationEvent::State(s) => {
                let steps: Vec<Step> = s.steps.into_iter().map(Step::of).collect();
                // The next step knows which exit a roundabout takes.
                let maneuver_type = match nav_maneuver_type_to_code(s.maneuver_type) {
                    Some(ManeuverType::EnterRoundabout) => steps
                        .first()
                        .and_then(|st| st.maneuver_type)
                        .or(Some(ManeuverType::EnterRoundabout)),
                    other => other,
                };
                let turn_angle = maneuver_type
                    .filter(|m| {
                        (ManeuverType::RoundaboutExit1 as u8..=ManeuverType::RoundaboutExit19 as u8)
                            .contains(&(*m as u8))
                    })
                    .and(steps.first().and_then(|st| st.turn_angle));
                let patch = Navigation {
                    maneuver_type,
                    turn_side: nav_maneuver_type_to_side(s.maneuver_type),
                    turn_angle,
                    road_name: s.road_name.filter(|r| !r.is_empty()),
                    destination_name: s.destination_address.filter(|d| !d.is_empty()),
                    steps,
                    ..Default::default()
                };
                if !patch.is_empty() {
                    self.publish(out, patch);
                }
            }
            NavigationEvent::Position(p) => {
                if p.step_distance_display
                    .as_deref()
                    .is_some_and(|d| !d.is_empty())
                {
                    self.navi.display_distance_e3 = None;
                    self.navi.display_distance_unit = None;
                }
                let patch = Navigation {
                    remain_distance: p.step_distance_meters,
                    time_to_turn: p.time_to_step_seconds,
                    step_distance_display: p.step_distance_display.filter(|d| !d.is_empty()),
                    step_distance_unit: p.step_distance_units,
                    distance_to_destination: p.destination_meters,
                    destination_distance_display: p.destination_display.filter(|d| !d.is_empty()),
                    destination_distance_unit: p.destination_units,
                    time_to_destination: p.time_to_arrival_seconds,
                    eta: p.eta_text.filter(|e| !e.is_empty()),
                    ..Default::default()
                };
                if !patch.is_empty() {
                    self.publish(out, patch);
                }
            }
        }
    }

    fn publish(&mut self, out: &mut Vec<BridgeOut>, patch: Navigation) {
        self.navi.merge(patch);
        if self.navi_app.is_some() && self.navi.app_name.is_none() {
            self.navi.app_name = self.navi_app.clone();
        }
        report(out, Report::Navigation(self.navi.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::navigation::{
        NavigationDistanceUpdate, NavigationPositionUpdate, NavigationStateUpdate,
        NavigationTurnEvent, NavigationTurnSide, NavigationTurnUpdate,
    };

    fn reports(out: Vec<BridgeOut>) -> Vec<Report> {
        out.into_iter()
            .filter_map(|o| match o {
                BridgeOut::Report(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn guidance_accumulates_and_starts_over() {
        let mut b = Bridge::default();
        let cfg = AaConfig::default();
        let nav = |b: &mut Bridge, e| reports(b.on_event(SessionEvent::Navigation(e), &cfg));
        let start = nav(&mut b, NavigationEvent::Start);
        assert_eq!(
            start,
            [Report::Navigation(Navigation {
                active: Some(true),
                app_name: Some("Google Maps".into()),
                ..Default::default()
            })]
        );
        let turn = NavigationTurnUpdate {
            road: Some("A9".into()),
            event: Some(NavigationTurnEvent::Turn),
            turn_side: Some(NavigationTurnSide::Right),
            image: Some(vec![1]),
            ..Default::default()
        };
        let out = nav(&mut b, NavigationEvent::Turn(turn));
        let Report::Navigation(n) = &out[0] else {
            panic!()
        };
        assert_eq!(n.maneuver_type, Some(ManeuverType::RightTurn));
        assert_eq!(n.turn_side, Some(DrivingSide::Right));
        assert_eq!(out[1], Report::NavigationImage(vec![1]));
        assert!(
            nav(
                &mut b,
                NavigationEvent::Turn(NavigationTurnUpdate::default())
            )
            .is_empty()
        );
        let out = nav(
            &mut b,
            NavigationEvent::Distance(NavigationDistanceUpdate {
                distance_meters: 80,
                ..Default::default()
            }),
        );
        let Report::Navigation(n) = &out[0] else {
            panic!()
        };
        assert_eq!(
            (n.remain_distance, n.road_name.as_deref()),
            (Some(80), Some("A9"))
        );
        let out = nav(
            &mut b,
            NavigationEvent::State(NavigationStateUpdate {
                maneuver_type: Some(41),
                road_name: Some(String::new()),
                ..Default::default()
            }),
        );
        let Report::Navigation(n) = &out[0] else {
            panic!()
        };
        assert_eq!(n.maneuver_type, Some(ManeuverType::ArrivedLeft));
        assert_eq!(n.road_name.as_deref(), Some("A9"));
        assert!(
            nav(
                &mut b,
                NavigationEvent::State(NavigationStateUpdate::default())
            )
            .is_empty()
        );
        assert!(
            nav(
                &mut b,
                NavigationEvent::Position(NavigationPositionUpdate::default())
            )
            .is_empty()
        );
        let out = nav(&mut b, NavigationEvent::Status(NavigationState::Rerouting));
        let Report::Navigation(n) = &out[0] else {
            panic!()
        };
        assert_eq!(n.active, Some(true));
        let out = nav(&mut b, NavigationEvent::Stop);
        let Report::Navigation(n) = &out[0] else {
            panic!()
        };
        assert_eq!(
            (n.active, n.app_name.as_deref(), n.road_name.as_deref()),
            (Some(false), Some("Google Maps"), Some("A9"))
        );
        let Report::Navigation(n) = &nav(&mut b, NavigationEvent::Start)[0] else {
            panic!()
        };
        assert_eq!((n.road_name.as_deref(), n.remain_distance), (None, None));

        b.on_event(SessionEvent::VideoFocusProjected, &cfg);
        let out = reports(b.on_event(SessionEvent::Disconnected("x".into()), &cfg));
        assert_eq!(
            out,
            [
                Report::VideoFocus {
                    cluster: false,
                    projected: false
                },
                Report::Disconnected
            ]
        );
        let out = nav(&mut b, NavigationEvent::Stop);
        assert_eq!(
            out,
            [Report::Navigation(Navigation {
                active: Some(false),
                ..Default::default()
            })]
        );
    }

    #[test]
    fn steps_keep_their_lanes_until_the_next_state() {
        use crate::channels::navigation::NavigationLaneDirection;

        let mut b = Bridge::default();
        let cfg = AaConfig::default();
        let nav = |b: &mut Bridge, e| reports(b.on_event(SessionEvent::Navigation(e), &cfg));
        let direction = |shape, highlighted| NavigationLaneDirection { shape, highlighted };
        let roundabout = NavigationStep {
            maneuver_type: Some(32),
            roundabout_exit_number: Some(2),
            road_name: Some("Ring".into()),
            lanes: vec![
                NavigationLane {
                    directions: vec![direction(2, false), direction(0, true)],
                },
                NavigationLane {
                    directions: vec![direction(1, true), direction(3, true)],
                },
            ],
            ..Default::default()
        };
        let then = NavigationStep {
            maneuver_type: Some(5),
            road_name: Some(String::new()),
            ..Default::default()
        };
        let out = nav(
            &mut b,
            NavigationEvent::State(NavigationStateUpdate {
                steps: vec![roundabout, then],
                ..Default::default()
            }),
        );
        let Report::Navigation(n) = &out[0] else {
            panic!()
        };
        assert_eq!(
            n.steps,
            [
                Step {
                    maneuver_type: Some(ManeuverType::RoundaboutExit2),
                    road_name: Some("Ring".into()),
                    lanes: vec![
                        Lane {
                            angles: vec![-45],
                            highlighted: None,
                            highlights: vec![],
                        },
                        Lane {
                            angles: vec![0, 45],
                            highlighted: Some(0),
                            highlights: vec![0, 45],
                        },
                    ],
                    ..Default::default()
                },
                Step {
                    maneuver_type: Some(ManeuverType::SlightLeft),
                    turn_side: Some(DrivingSide::Left),
                    ..Default::default()
                },
            ]
        );
        let out = nav(
            &mut b,
            NavigationEvent::Position(NavigationPositionUpdate {
                step_distance_meters: Some(50),
                ..Default::default()
            }),
        );
        let Report::Navigation(n) = &out[0] else {
            panic!()
        };
        assert_eq!(n.steps.len(), 2);
    }

    #[test]
    fn roundabout_exits_times_and_phone_text_are_kept() {
        let mut b = Bridge::default();
        let cfg = AaConfig::default();
        let nav = |b: &mut Bridge, e| reports(b.on_event(SessionEvent::Navigation(e), &cfg));
        let last = |out: Vec<Report>| match out.into_iter().next() {
            Some(Report::Navigation(n)) => n,
            other => panic!("{other:?}"),
        };
        nav(&mut b, NavigationEvent::Start);
        let n = last(nav(
            &mut b,
            NavigationEvent::Turn(NavigationTurnUpdate {
                event: Some(NavigationTurnEvent::RoundaboutEnterAndExit),
                turn_side: Some(NavigationTurnSide::Right),
                turn_number: Some(3),
                turn_angle: Some(270),
                ..Default::default()
            }),
        ));
        assert_eq!(n.maneuver_type, Some(ManeuverType::RoundaboutExit3));
        assert_eq!(n.turn_angle, Some(270));

        let n = last(nav(
            &mut b,
            NavigationEvent::Distance(NavigationDistanceUpdate {
                distance_meters: 350,
                time_to_turn_seconds: 21,
                display_distance_e3: Some(350_000),
                display_unit: Some(1),
            }),
        ));
        assert_eq!(
            (
                n.time_to_turn,
                n.display_distance_e3,
                n.display_distance_unit
            ),
            (Some(21), Some(350_000), Some(1))
        );

        let n = last(nav(
            &mut b,
            NavigationEvent::Position(NavigationPositionUpdate {
                step_distance_meters: Some(300),
                step_distance_display: Some("300 m".into()),
                step_distance_units: Some(1),
                time_to_step_seconds: Some(18),
                destination_meters: Some(12_000),
                destination_display: Some("12 km".into()),
                destination_units: Some(2),
                ..Default::default()
            }),
        ));
        assert_eq!(n.time_to_turn, Some(18));
        assert_eq!(n.display_distance_e3, None);
        assert_eq!(
            (n.step_distance_display.as_deref(), n.step_distance_unit),
            (Some("300 m"), Some(1))
        );
        assert_eq!(
            (
                n.destination_distance_display.as_deref(),
                n.destination_distance_unit
            ),
            (Some("12 km"), Some(2))
        );

        // A state names the roundabout; its first step says which exit.
        let n = last(nav(
            &mut b,
            NavigationEvent::State(NavigationStateUpdate {
                maneuver_type: Some(32),
                steps: vec![NavigationStep {
                    maneuver_type: Some(32),
                    roundabout_exit_number: Some(2),
                    roundabout_exit_angle: Some(180),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        ));
        assert_eq!(n.maneuver_type, Some(ManeuverType::RoundaboutExit2));
        assert_eq!(n.turn_angle, Some(180));
    }

    #[test]
    fn focus_media_and_audio() {
        let mut b = Bridge::default();
        let cfg = AaConfig {
            cluster_tier_width: Some(800),
            cluster_tier_height: Some(480),
            ..Default::default()
        };
        let out = b.on_event(SessionEvent::VideoStarted, &cfg);
        assert_eq!(
            out,
            [
                BridgeOut::Report(Report::VideoFocus {
                    cluster: false,
                    projected: true
                }),
                BridgeOut::VideoStarted {
                    cluster: false,
                    width: 1280,
                    height: 720
                },
            ]
        );
        assert_eq!(b.on_event(SessionEvent::VideoStarted, &cfg).len(), 1);
        let out = b.on_event(SessionEvent::ClusterVideoStarted, &cfg);
        assert_eq!(
            out[1],
            BridgeOut::VideoStarted {
                cluster: true,
                width: 800,
                height: 480
            }
        );
        assert_eq!(
            reports(b.on_event(SessionEvent::AudioFocus(3), &cfg)),
            [Report::Duck {
                level: 0.2,
                duration_ms: 500
            }]
        );
        assert_eq!(
            reports(b.on_event(SessionEvent::AudioFocus(1), &cfg)),
            [Report::Duck {
                level: 1.0,
                duration_ms: 1500
            }]
        );
        let meta = MediaPlaybackMetadata {
            album_art: Some(Vec::new()),
            ..Default::default()
        };
        assert!(
            b.on_event(SessionEvent::MediaMetadata(meta), &cfg)
                .is_empty()
        );
        let meta = MediaPlaybackMetadata {
            song: Some("S".into()),
            duration_seconds: Some(3),
            album_art: Some(vec![9]),
            ..Default::default()
        };
        assert_eq!(
            reports(b.on_event(SessionEvent::MediaMetadata(meta), &cfg)),
            [Report::NowPlaying(NowPlaying {
                title: Some("S".into()),
                duration_ms: Some(3000),
                artwork: Some(vec![9]),
                ..Default::default()
            })]
        );
        let status = MediaPlaybackStatus {
            playback_seconds: Some(2),
            ..Default::default()
        };
        assert_eq!(
            reports(b.on_event(SessionEvent::MediaStatus(status), &cfg)),
            [Report::NowPlaying(NowPlaying {
                playing: Some(false),
                elapsed_ms: Some(2000),
                ..Default::default()
            })]
        );
        assert_eq!(
            b.on_event(
                SessionEvent::AudioSetup {
                    channel: AudioChannelType::System,
                    sample_rate: 16000,
                    channels: 1
                },
                &cfg
            ),
            [BridgeOut::PrimeAudio {
                kind: AudioKind::Alert,
                sample_rate: 16000,
                channels: 1,
                tag: AudioChannelType::System
            }]
        );
        assert_eq!(
            b.on_event(SessionEvent::VoiceSession(true), &cfg),
            [BridgeOut::StartMic("voice-session START")]
        );
        assert_eq!(
            b.on_event(SessionEvent::MicStop, &cfg),
            [BridgeOut::StopMic("mic-stop")]
        );
        assert_eq!(
            b.on_event(SessionEvent::Connected, &cfg),
            [BridgeOut::Report(Report::Connected), BridgeOut::AudioSinks]
        );
    }
}
