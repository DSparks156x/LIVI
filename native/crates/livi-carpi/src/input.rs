// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpid's input actions for the projection session, PLAN.md 2.4: Android Auto commands (sent
//! as keycodes on its input channel) and CarPlay stack commands. livi-core's projection sends
//! them to the active session; `OpenPage` is no session's and maps to nothing here.
//!
//! Select and voice are real holds: down and up arrive as two actions and go out as two. The rest
//! are presses.

use carpi_proto::InputAction;
use livi_aa_stack::commands::Command;
use livi_cp::hid::{KnobState, media_button, telephony_button};
use livi_cp::stack::StackCmd;

/// Most detents one `Rotate` turns.
pub const MAX_STEPS: u32 = 16;
/// Full knob deflection, as LIVI's own CarPlay keys use.
const DEFLECT: f64 = 127.0;

fn steps(n: i32) -> usize {
    n.unsigned_abs().min(MAX_STEPS) as usize
}

/// Android Auto commands for `action`, in order.
pub fn aa(action: &InputAction) -> Vec<Command> {
    use InputAction as A;
    let c = match action {
        A::Up => Command::Up,
        A::Down => Command::Down,
        A::Left => Command::Left,
        A::Right => Command::Right,
        A::Rotate { steps: n } => {
            let c = if *n > 0 {
                Command::KnobRight
            } else {
                Command::KnobLeft
            };
            return vec![c; steps(*n)];
        }
        A::Select { down: true } => Command::SelectDown,
        A::Select { down: false } => Command::SelectUp,
        A::Back => Command::Back,
        A::Home => Command::Home,
        A::PlayPause => Command::PlayPause,
        A::Next => Command::Next,
        A::Prev => Command::Prev,
        A::CallAccept => Command::AcceptPhone,
        A::CallEnd => Command::RejectPhone,
        A::Voice { down: true } => Command::VoiceAssistant,
        A::Voice { down: false } => Command::VoiceAssistantRelease,
        A::OpenNav => Command::OpenNavigation,
        A::OpenPhone => Command::OpenPhone,
        A::OpenMedia => Command::OpenMedia,
        A::OpenPage { .. } => return Vec::new(),
    };
    vec![c]
}

fn knob(state: KnobState) -> StackCmd {
    StackCmd::Knob {
        state,
        momentary: true,
    }
}

/// CarPlay stack commands for `action`, in order.
pub fn cp(action: &InputAction) -> Vec<StackCmd> {
    use InputAction as A;
    let rest = KnobState::default();
    let c = match action {
        A::Up => knob(KnobState {
            y: -DEFLECT,
            ..rest
        }),
        A::Down => knob(KnobState { y: DEFLECT, ..rest }),
        A::Left => knob(KnobState {
            x: -DEFLECT,
            ..rest
        }),
        A::Right => knob(KnobState { x: DEFLECT, ..rest }),
        A::Rotate { steps: n } => {
            let wheel = if *n > 0 { 1.0 } else { -1.0 };
            return vec![knob(KnobState { wheel, ..rest }); steps(*n)];
        }
        A::Select { down } => StackCmd::KnobSelect(*down),
        A::Back => knob(KnobState { back: true, ..rest }),
        A::Home => knob(KnobState { home: true, ..rest }),
        A::PlayPause => StackCmd::Media(media_button::PLAY_PAUSE),
        A::Next => StackCmd::Media(media_button::NEXT),
        A::Prev => StackCmd::Media(media_button::PREV),
        A::CallAccept => StackCmd::Telephony(telephony_button::HOOK_SWITCH),
        A::CallEnd => StackCmd::Telephony(telephony_button::DROP),
        A::Voice { down } => StackCmd::SiriButton(*down),
        A::OpenNav => StackCmd::RequestUi("maps:".into()),
        A::OpenPhone => StackCmd::RequestUi("mobilephone:".into()),
        A::OpenMedia => StackCmd::RequestUi("nowplaying:".into()),
        A::OpenPage { .. } => return Vec::new(),
    };
    vec![c]
}

/// The open-app actions also bring the projection to the front.
pub fn shows_projection(action: &InputAction) -> bool {
    matches!(
        action,
        InputAction::OpenNav | InputAction::OpenPhone | InputAction::OpenMedia
    )
}

/// LIVI's UI path for `OpenPage { page }`, `None` for a page the fork doesn't have.
pub fn page_path(page: &str) -> Option<String> {
    livi_carpi_ui::page_path(page)
}

#[cfg(test)]
mod tests {
    use livi_aa_proto::Keycode;
    use livi_aa_stack::commands::{Action, command_action};

    use super::*;

    fn every() -> Vec<InputAction> {
        use InputAction as A;
        vec![
            A::Up,
            A::Down,
            A::Left,
            A::Right,
            A::Rotate { steps: 2 },
            A::Rotate { steps: -1 },
            A::Select { down: true },
            A::Select { down: false },
            A::Back,
            A::Home,
            A::PlayPause,
            A::Next,
            A::Prev,
            A::CallAccept,
            A::CallEnd,
            A::Voice { down: true },
            A::Voice { down: false },
            A::OpenNav,
            A::OpenPhone,
            A::OpenMedia,
        ]
    }

    #[test]
    fn android_auto_gets_the_keycodes_of_the_table() {
        use InputAction as A;
        let key = |a: A| -> Vec<Action> { aa(&a).into_iter().map(command_action).collect() };
        let click = |k: Keycode| vec![Action::Click(k as u32)];
        assert_eq!(key(A::OpenNav), click(Keycode::Navigation));
        assert_eq!(key(A::OpenPhone), click(Keycode::Tel));
        assert_eq!(key(A::OpenMedia), click(Keycode::Media));
        assert_eq!(key(A::CallAccept), click(Keycode::Call));
        assert_eq!(key(A::CallEnd), click(Keycode::Endcall));
        assert_eq!(key(A::PlayPause), click(Keycode::MediaPlayPause));
        assert_eq!(key(A::Next), click(Keycode::MediaNext));
        assert_eq!(key(A::Prev), click(Keycode::MediaPrevious));
        assert_eq!(key(A::Back), click(Keycode::Back));
        assert_eq!(key(A::Home), click(Keycode::Home));
        assert_eq!(key(A::Up), click(Keycode::DpadUp));
        assert_eq!(key(A::Down), click(Keycode::DpadDown));
        assert_eq!(
            key(A::Rotate { steps: 2 }),
            [Action::Rotary(1), Action::Rotary(1)]
        );
        assert_eq!(key(A::Rotate { steps: -1 }), [Action::Rotary(-1)]);
        assert_eq!(key(A::Rotate { steps: 0 }), []);
        assert_eq!(aa(&A::Rotate { steps: 900 }).len(), MAX_STEPS as usize);
    }

    #[test]
    fn holds_go_down_then_up() {
        use InputAction as A;
        let one = |a: A| command_action(aa(&a)[0]);
        let center = Keycode::DpadCenter as u32;
        let voice = Keycode::Search as u32;
        assert_eq!(
            one(A::Select { down: true }),
            Action::Key {
                code: center,
                down: true
            }
        );
        assert_eq!(
            one(A::Select { down: false }),
            Action::Key {
                code: center,
                down: false
            }
        );
        assert_eq!(
            one(A::Voice { down: true }),
            Action::Key {
                code: voice,
                down: true
            }
        );
        assert_eq!(
            one(A::Voice { down: false }),
            Action::Key {
                code: voice,
                down: false
            }
        );
        assert_eq!(cp(&A::Voice { down: true }), [StackCmd::SiriButton(true)]);
        assert_eq!(cp(&A::Voice { down: false }), [StackCmd::SiriButton(false)]);
        assert_eq!(cp(&A::Select { down: true }), [StackCmd::KnobSelect(true)]);
    }

    #[test]
    fn carplay_gets_knob_media_telephony_and_apps() {
        use InputAction as A;
        assert_eq!(cp(&A::OpenNav), [StackCmd::RequestUi("maps:".into())]);
        assert_eq!(
            cp(&A::OpenPhone),
            [StackCmd::RequestUi("mobilephone:".into())]
        );
        assert_eq!(
            cp(&A::OpenMedia),
            [StackCmd::RequestUi("nowplaying:".into())]
        );
        assert_eq!(cp(&A::Next), [StackCmd::Media(media_button::NEXT)]);
        assert_eq!(
            cp(&A::CallEnd),
            [StackCmd::Telephony(telephony_button::DROP)]
        );
        let StackCmd::Knob { state, momentary } = cp(&A::Up)[0].clone() else {
            panic!("up is a knob move");
        };
        assert!(momentary);
        assert!(state.y < 0.0);
        let turns = cp(&A::Rotate { steps: -3 });
        assert_eq!(turns.len(), 3);
        assert!(
            turns
                .iter()
                .all(|t| matches!(t, StackCmd::Knob { state, .. } if state.wheel == -1.0))
        );
        let StackCmd::Knob { state, .. } = cp(&A::Back)[0].clone() else {
            panic!("back is a knob button");
        };
        assert!(state.back && !state.home);
    }

    #[test]
    fn every_action_but_pages_maps_for_both() {
        for a in every() {
            assert!(!aa(&a).is_empty(), "{a:?} for Android Auto");
            assert!(!cp(&a).is_empty(), "{a:?} for CarPlay");
        }
        let page = InputAction::OpenPage {
            page: "light".into(),
        };
        assert!(aa(&page).is_empty() && cp(&page).is_empty());
        assert!(shows_projection(&InputAction::OpenNav));
        assert!(!shows_projection(&InputAction::Back));
    }
}
