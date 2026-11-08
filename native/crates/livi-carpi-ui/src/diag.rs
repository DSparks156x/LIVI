// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The diagnostics page's data from carpid's diag API (PLAN.md 2.8): the install list and fault
//! codes come from `Request::Diag` jobs, measuring-group values from a subscription to the diag
//! catalog's names (`carpi-diag/src/catalog.rs`), which makes carpid poll them while the page
//! shows them.

use std::collections::BTreeMap;

use carpi_proto::{DiagAction, InstalledModule, JobEvent};

/// A module the page lists: VCDS address and TP2 ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// VCDS address, the one people know.
    pub address: u8,
    /// TP2 ID behind the gateway: what the diag jobs take.
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

/// A module's name by VCDS address.
pub fn name_of(address: u8) -> &'static str {
    DEFAULT_MODULES
        .iter()
        .find(|m| m.0 == address)
        .map_or("Module", |m| m.2)
}

/// carpid never polls this one: polling ABS upsets ESP and the Haldex.
pub const NO_POLL: u8 = 0x03;

/// The diag catalog's values that only diagnostics give: name, TP2 module, group and field of the
/// first location. carpid's scheduler may read them from another group.
pub const MEASURED: &[(&str, u8, u8, u8)] = &[
    ("engine.injection_ms", 0x01, 2, 3),
    ("engine.maf_gs", 0x01, 3, 2),
    ("engine.throttle_pct", 0x01, 3, 3),
    ("engine.ignition_timing_deg", 0x01, 3, 4),
    ("engine.terminal30_v", 0x01, 4, 2),
    ("engine.intake_temp_c", 0x01, 4, 4),
    ("engine.timing_retard.cyl1_deg", 0x01, 20, 1),
    ("engine.timing_retard.cyl2_deg", 0x01, 20, 2),
    ("engine.timing_retard.cyl3_deg", 0x01, 20, 3),
    ("engine.timing_retard.cyl4_deg", 0x01, 20, 4),
    ("engine.lambda.actual", 0x01, 31, 1),
    ("engine.lambda.specified", 0x01, 31, 2),
    ("engine.pedal_pct", 0x01, 62, 3),
    ("engine.fuel_rail.specified_bar", 0x01, 106, 1),
    ("engine.fuel_rail.actual_bar", 0x01, 106, 2),
    ("engine.load_pct", 0x01, 113, 2),
    ("engine.atmospheric_mbar", 0x01, 113, 4),
    ("engine.n75_duty_pct", 0x01, 114, 4),
    ("engine.boost.specified", 0x01, 115, 3),
    ("engine.boost.actual", 0x01, 115, 4),
    ("engine.oil_temp_c", 0x01, 134, 1),
    ("awd.oil_temp_c", 0x0A, 1, 1),
    ("awd.plate_temp_c", 0x0A, 1, 2),
    ("awd.supply_v", 0x0A, 1, 3),
    ("awd.oil_pressure_bar", 0x0A, 3, 1),
    ("awd.estimated_torque_nm", 0x0A, 3, 2),
    ("awd.valve_opening_pct", 0x0A, 3, 3),
    ("awd.valve_current", 0x0A, 3, 4),
];

/// The catalog values of TP2 module `tp2`, by group.
pub fn measured(tp2: u8) -> impl Iterator<Item = &'static (&'static str, u8, u8, u8)> {
    MEASURED.iter().filter(move |m| m.1 == tp2)
}

/// One entry of the gateway's install list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub module: Module,
    /// From the install list; `None` on the default list.
    pub coded: Option<bool>,
    pub status: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dtc {
    /// Five decimal digits, as VAG shows them.
    pub code: String,
    /// The raw status byte, meaning unknown.
    pub status: u8,
}

/// The diag job this connection runs, or how the last one ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Running {
        job: u32,
        action: DiagAction,
        stage: String,
    },
    Done {
        action: DiagAction,
        ok: bool,
        message: String,
    },
}

/// What carpid told about diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diag {
    pub installed: Option<Vec<Installed>>,
    /// Fault codes per TP2 module, after a read or a clear.
    pub dtcs: BTreeMap<u8, Vec<Dtc>>,
    pub job: Option<Job>,
}

fn module_of(action: DiagAction) -> Option<u8> {
    match action {
        DiagAction::ReadDtcs { module } | DiagAction::ClearDtcs { module, .. } => Some(module),
        DiagAction::InstallList | DiagAction::Status => None,
    }
}

fn doing(action: DiagAction) -> &'static str {
    match action {
        DiagAction::InstallList => "Reading the install list",
        DiagAction::ReadDtcs { .. } => "Reading fault codes",
        DiagAction::ClearDtcs { .. } => "Clearing fault codes",
        DiagAction::Status => "Reading the diag status",
    }
}

impl Diag {
    /// A job is running: one at a time from the pages.
    pub fn busy(&self) -> bool {
        matches!(self.job, Some(Job::Running { .. }))
    }

    pub fn started(&mut self, job: u32, action: DiagAction) {
        self.job = Some(Job::Running {
            job,
            action,
            stage: String::new(),
        });
    }

    /// carpid refused to start `action`.
    pub fn refused(&mut self, action: DiagAction, message: String) {
        self.job = Some(Job::Done {
            action,
            ok: false,
            message,
        });
    }

    /// One event of job `job`; others' are ignored.
    pub fn event(&mut self, job: u32, event: JobEvent) {
        let Some(Job::Running {
            job: ours,
            action,
            stage,
        }) = &mut self.job
        else {
            return;
        };
        if *ours != job {
            return;
        }
        match event {
            JobEvent::Stage { stage: s, .. } => *stage = s,
            JobEvent::InstallList { modules } => {
                self.installed = Some(modules.iter().map(installed).collect());
            }
            JobEvent::Dtcs { module, dtcs } => {
                let codes = dtcs
                    .iter()
                    .map(|d| Dtc {
                        code: format!("{:05}", d.code),
                        status: d.status,
                    })
                    .collect();
                self.dtcs.insert(module, codes);
            }
            JobEvent::Finished { ok, message } => {
                let action = *action;
                self.job = Some(Job::Done {
                    action,
                    ok,
                    message,
                });
            }
            _ => {}
        }
    }

    /// A line about the running or last job, for `module` or for any with `None`.
    pub fn line(&self, module: Option<u8>) -> Option<String> {
        let (action, text) = match self.job.as_ref()? {
            Job::Running { action, stage, .. } if stage.is_empty() => {
                (*action, format!("{}…", doing(*action)))
            }
            Job::Running { action, stage, .. } => {
                (*action, format!("{}: {stage}…", doing(*action)))
            }
            Job::Done {
                action,
                ok: true,
                message,
            } if message.is_empty() => (*action, format!("{}: done.", doing(*action))),
            Job::Done {
                action,
                ok: true,
                message,
            } => (*action, format!("{}: {message}", doing(*action))),
            Job::Done {
                action, message, ..
            } => (*action, format!("{} failed: {message}", doing(*action))),
        };
        let mine = match (module, module_of(action)) {
            (Some(m), Some(a)) => m == a,
            (Some(_), None) => false,
            (None, _) => true,
        };
        mine.then_some(text)
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
                    coded: None,
                    status: None,
                })
                .collect(),
        }
    }
}

fn installed(m: &InstalledModule) -> Installed {
    Installed {
        module: Module {
            address: m.vcds,
            tp2: m.tp2,
            name: name_of(m.vcds).into(),
        },
        coded: Some(m.coded),
        status: Some(m.status),
    }
}

#[cfg(test)]
mod tests {
    use carpi_proto::Dtc as WireDtc;

    use super::*;

    #[test]
    fn jobs_fill_the_list_and_the_codes() {
        let mut d = Diag::default();
        assert_eq!(d.modules().len(), DEFAULT_MODULES.len());
        d.started(4, DiagAction::InstallList);
        assert!(d.busy());
        assert_eq!(d.line(None).as_deref(), Some("Reading the install list…"));
        d.event(
            4,
            JobEvent::InstallList {
                modules: vec![InstalledModule {
                    vcds: 0x22,
                    tp2: 0x0A,
                    coded: true,
                    status: 0x10,
                }],
            },
        );
        d.event(
            9,
            JobEvent::Finished {
                ok: false,
                message: "not ours".into(),
            },
        );
        assert!(d.busy());
        d.event(
            4,
            JobEvent::Finished {
                ok: true,
                message: String::new(),
            },
        );
        assert!(!d.busy());
        assert_eq!(d.modules()[0].module.name, "AWD");
        assert_eq!(d.modules()[0].coded, Some(true));

        d.started(5, DiagAction::ReadDtcs { module: 1 });
        d.event(
            5,
            JobEvent::Dtcs {
                module: 1,
                dtcs: vec![WireDtc {
                    code: 1314,
                    status: 0x23,
                }],
            },
        );
        assert_eq!(d.dtcs[&1][0].code, "01314");
        assert_eq!(d.line(Some(0x0A)), None);
        assert!(d.line(Some(1)).unwrap().starts_with("Reading fault codes"));
        d.refused(
            DiagAction::ClearDtcs {
                module: 1,
                confirm: true,
            },
            "no diag role".into(),
        );
        assert_eq!(
            d.line(Some(1)).as_deref(),
            Some("Clearing fault codes failed: no diag role")
        );
    }

    #[test]
    fn the_catalog_rows_are_diag_only_and_never_abs() {
        assert_eq!(measured(0x01).count(), 21);
        assert_eq!(measured(0x0A).count(), 7);
        assert_eq!(measured(NO_POLL).count(), 0);
        for (i, m) in MEASURED.iter().enumerate() {
            assert!(MEASURED[i + 1..].iter().all(|n| n.0 != m.0), "{}", m.0);
        }
    }
}
