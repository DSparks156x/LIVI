// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The diagnostics page's data, in the shape carpid's diag API is expected to take (PLAN.md 2.8:
//! install list, DTC read/clear as jobs, measuring groups through the scheduler).
//!
//! carpid has none of it yet, so everything here stays `None` and the page says "not available
//! yet". When the API lands, fill [`Diag`] from it; the page already lays it out.

use std::collections::BTreeMap;

/// A module the page lists: VCDS address and TP2 ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// VCDS address, the one people know.
    pub address: u8,
    /// TP2 ID behind the gateway.
    pub tp2: u8,
    pub name: String,
}

/// Until the install list: the list the old DataView fell back to.
pub const DEFAULT_MODULES: &[(u8, u8, &str)] = &[
    (0x01, 0x01, "Engine"),
    (0x02, 0x02, "Auto trans"),
    (0x03, 0x03, "ABS brakes"),
    (0x08, 0x2C, "Auto HVAC"),
    (0x09, 0x20, "Central electrics"),
    (0x0F, 0x4F, "Digital radio"),
    (0x15, 0x05, "Airbags"),
    (0x16, 0x2A, "Steering wheel"),
    (0x17, 0x07, "Instruments"),
    (0x19, 0x1F, "CAN gateway"),
    (0x22, 0x0A, "AWD"),
    (0x25, 0x14, "Immobilizer"),
    (0x37, 0x5B, "Navigation"),
    (0x42, 0x22, "Door electronics, driver"),
    (0x44, 0x09, "Steering assist"),
    (0x46, 0x21, "Central convenience"),
    (0x52, 0x23, "Door electronics, passenger"),
    (0x55, 0x06, "Headlight range"),
    (0x56, 0x52, "Radio"),
    (0x65, 0x29, "Tyre pressure"),
];

/// One entry of the gateway's install list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub module: Module,
    pub part: Option<String>,
    pub component: Option<String>,
    pub coding: Option<String>,
    /// Faults stored, when the list says.
    pub faults: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dtc {
    pub code: String,
    pub text: String,
    /// Status bits decoded, e.g. "intermittent".
    pub status: String,
}

/// One field of a measuring group, formatted by the ECU's formula byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub value: String,
    pub unit: String,
}

/// Groups shown per module, like the old page.
pub const GROUP_SLOTS: usize = 3;

/// What carpid told about diagnostics. All `None` until it has a diag API.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diag {
    pub installed: Option<Vec<Installed>>,
    /// Fault codes per VCDS address, after a read.
    pub dtcs: BTreeMap<u8, Vec<Dtc>>,
    /// Measuring groups per (VCDS address, group).
    pub groups: BTreeMap<(u8, u8), Vec<Field>>,
}

impl Diag {
    /// Whether carpid can do diagnostics at all.
    pub fn available(&self) -> bool {
        self.installed.is_some()
    }

    /// The install list, or the default list without any detail.
    pub fn modules(&self) -> Vec<Installed> {
        match &self.installed {
            Some(list) => list.clone(),
            None => DEFAULT_MODULES
                .iter()
                .map(|&(address, tp2, name)| Installed {
                    module: Module {
                        address,
                        tp2,
                        name: name.into(),
                    },
                    part: None,
                    component: None,
                    coding: None,
                    faults: None,
                })
                .collect(),
        }
    }
}
