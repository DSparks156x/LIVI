// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpi fork: carpid's input actions for the active session, mapped by livi-carpi.

use super::*;

impl Projection {
    pub(super) fn carpi_input(&self, action: &livi_carpi::InputAction) {
        match self.active().map(|s| s.link) {
            Some(Link::Aa(id)) => {
                for c in livi_carpi::input::aa(action) {
                    self.aa.session(id, SessionCmd::Command(c));
                }
            }
            Some(Link::Cp(id)) => {
                for c in livi_carpi::input::cp(action) {
                    self.cp.stack(id, c);
                }
            }
            None => println!("[carpi] input {action:?} dropped: the session ended"),
        }
    }
}
