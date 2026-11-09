// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The car's keys and the binding table in plain words, for the Controls screens. Names as
//! carpid has them (`carpi-input/src/names.rs`).

use carpi_proto::Binding;

pub struct KeyName {
    pub key: &'static str,
    pub label: &'static str,
    /// On the steering wheel; the rest are the RNS-E's faceplate.
    pub wheel: bool,
}

const fn k(key: &'static str, label: &'static str, wheel: bool) -> KeyName {
    KeyName { key, label, wheel }
}

/// In the order the screens list them.
pub const KEYS: &[KeyName] = &[
    k("wheel_up", "Roller up", true),
    k("wheel_down", "Roller down", true),
    k("wheel_click", "Roller push", true),
    k("mode", "Mode", true),
    k("ptt", "Voice", true),
    k("vol_up", "Volume up", true),
    k("vol_down", "Volume down", true),
    k("vol_click", "Volume push", true),
    k("stalk_up", "Stalk rocker up", true),
    k("stalk_down", "Stalk rocker down", true),
    k("knob_cw", "Knob right", false),
    k("knob_ccw", "Knob left", false),
    k("enter", "Knob push", false),
    k("return", "Return", false),
    k("prev", "Previous", false),
    k("next", "Next", false),
    k("up", "Up", false),
    k("down", "Down", false),
    k("setup", "Setup", false),
    k("nav", "Nav", false),
    k("tel", "Tel", false),
    k("media", "Media", false),
    k("name", "Name", false),
    k("info", "Info", false),
    k("car", "Car", false),
];

pub fn key_at(key: &str) -> Option<usize> {
    KEYS.iter().position(|k| k.key == key)
}

pub fn key_label(key: &str) -> String {
    KEYS.iter()
        .find(|k| k.key == key)
        .map_or_else(|| words(key), |k| k.label.to_string())
}

/// The keys of one source that have bindings, in [`KEYS`] order.
pub fn bound(bindings: &[Binding], wheel: bool) -> Vec<usize> {
    KEYS.iter()
        .enumerate()
        .filter(|(_, k)| k.wheel == wheel && bindings.iter().any(|b| b.key == k.key))
        .map(|(i, _)| i)
        .collect()
}

/// `held_s`: how long the "extended" hold is.
pub fn gesture_label(g: &str, held_s: u32) -> String {
    match g {
        "short" => "Short".into(),
        "long" => "Long".into(),
        "extended" => format!("Held {held_s} s"),
        "double" => "Double".into(),
        "detent" => "Turn".into(),
        other => words(other),
    }
}

/// When a table's rows apply.
pub fn table_label(t: &str) -> String {
    match t {
        "dis_toggle" | "system" => "always".into(),
        "call_ringing" => "while ringing".into(),
        "call_active" => "in a call".into(),
        "dis" => "on the cluster".into(),
        "car" => "for the car".into(),
        "livi" => "on screen".into(),
        other => words(other),
    }
}

pub fn action_label(a: &str) -> String {
    match a {
        "toggle" => "Cluster screen on/off".into(),
        "toggle_dis_focus" => "Keys to the cluster".into(),
        "shutdown" => "Shut down".into(),
        "reboot" => "Restart the Pi".into(),
        "save_capture" => "Capture".into(),
        "brightness_up" => "Brighter".into(),
        "brightness_down" => "Dimmer".into(),
        "toggle_logging" => "Start/stop logging".into(),
        "open_nav" => "Open nav".into(),
        "open_phone" => "Open phone".into(),
        "open_media" => "Open media".into(),
        "open_page_data" => "Open Data".into(),
        "open_page_car" => "Open Car Integration".into(),
        "call_accept" | "accept_phone" => "Answer".into(),
        "call_end" | "reject_phone" => "Hang up".into(),
        "play_pause" => "Play/pause".into(),
        "prev" => "Previous track".into(),
        "next" => "Next track".into(),
        "knob_left" => "Knob left".into(),
        "knob_right" => "Knob right".into(),
        "cycle_session" => "Next phone".into(),
        other => {
            if let Some(code) = other.strip_prefix("code:") {
                format!("Key code {code}")
            } else {
                words(other)
            }
        }
    }
}

/// `wheel_up` as "Wheel up".
pub fn words(s: &str) -> String {
    let s = s.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_split_by_source() {
        let b = |key: &str| Binding {
            table: "livi".into(),
            key: key.into(),
            gesture: "short".into(),
            action: "up".into(),
        };
        let rows = vec![b("wheel_up"), b("enter"), b("mode"), b("enter")];
        let wheel: Vec<&str> = bound(&rows, true).iter().map(|i| KEYS[*i].key).collect();
        assert_eq!(wheel, ["wheel_up", "mode"]);
        let face: Vec<&str> = bound(&rows, false).iter().map(|i| KEYS[*i].key).collect();
        assert_eq!(face, ["enter"]);
        assert_eq!(action_label("open_page_data"), "Open Data");
        assert_eq!(action_label("select"), "Select");
        assert_eq!(gesture_label("extended", 10), "Held 10 s");
    }
}
