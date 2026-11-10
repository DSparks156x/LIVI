// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pages' own connection to carpid, on a thread of its own.
//!
//! Same socket and protocol as livi-carpi (`$CARPID_SOCKET` or `$XDG_RUNTIME_DIR/carpid.sock`,
//! length-prefixed JSON). It checks flashing status even with the pages hidden, so the mode stays
//! visible over projection. While a page is on screen: a subscription to every value, plus
//! `Values` and `Bus` once a second so ages stay
//! true, and carpid's settings each time a page comes up. The diagnostics page adds a second
//! subscription, to the diag catalog's names it shows, and runs diag jobs on this connection.
//! Data logging's status comes once a second too, with the values.
//! `CARPI_UI_FIXTURE=<file.json>` shows a fixed set instead, for snapshots without carpid.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use carpi_proto::{
    BusInfo, DatalogAction, DatalogStatus, DiagAction, JobEvent, LightAction, Reading, Settings,
    SettingsPatch, ToastLevel, UpdateAction,
};
use serde::Deserialize;

use crate::diag::Diag;
use crate::values::Store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    Idle,
    Saving,
    /// carpid's `2026-11-08 #03` and what went in.
    Saved {
        label: String,
        summary: String,
    },
    Failed(String),
}

/// A short message over whatever is on screen. `seq` tells a new one from the last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub seq: u64,
    pub text: String,
    pub level: ToastLevel,
}

/// What the pages know about carpid.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Kept current even while our pages are hidden; retained during a disconnection.
    pub flashing_mode: bool,
    pub connected: bool,
    /// carpid's name and version from its hello.
    pub daemon: Option<String>,
    /// Why there is no connection, or carpid's last error.
    pub problem: Option<String>,
    pub values: Store,
    pub buses: Option<Vec<BusInfo>>,
    pub capture: Capture,
    /// The install list, fault codes and diag jobs.
    pub diag: Diag,
    /// carpid's settings, from the last `Settings` reply.
    pub settings: Option<Settings>,
    /// Settings saved that only take effect after a restart of carpid.
    pub restart_needed: Vec<String>,
    /// Why the last settings change didn't go through, or a note about one.
    pub settings_note: Option<String>,
    /// The last toast the pages raised.
    pub toast: Option<Toast>,
    /// Data-log profiles and the run, from carpid.
    pub datalog: Option<DatalogStatus>,
    /// Why the last data-log ask didn't go through.
    pub datalog_note: Option<String>,
    /// The last update job this connection started.
    pub update: UpdateJob,
    /// Persisted release state from the updater, including after carpid restarts.
    pub update_status: Option<carpi_proto::Installed>,
    pub(crate) update_recovering: bool,
    /// Status replies started before a new update must not finish that update.
    pub(crate) update_generation: u64,
    /// Evidence this attempt reached apply; persisted terminal outcomes alone are history.
    pub(crate) update_apply_observed: bool,
    /// Only a request made by this UI creates an attempt; startup polling is passive.
    pub(crate) update_attempt: Option<UpdateAttempt>,
    /// An unresolved local attempt is information, not a confirmed failure.
    pub(crate) update_note: Option<String>,
    /// Saved Wi-Fi networks and the last scan or test.
    pub wifi: Wifi,
    /// Where the system runs from, and Move to NVMe.
    pub storage: Storage,
    /// The last requested installed decoder CRC read, including progress/errors.
    pub decoder: UpdateJob,
    /// SDLink firmware as carpid last told it: its `Firmware` status reply, or the
    /// `JobEvent::Firmware` of a firmware job. Cleared when carpid goes away.
    pub firmware: crate::firmware::Status,
    /// The last firmware probe or install this connection started.
    pub firmware_job: FirmwareJob,
}

/// A firmware probe or install: one at a time (carpid refuses a second).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FirmwareJob {
    /// What was asked; `None` before the first.
    pub action: Option<carpi_proto::FirmwareAction>,
    pub job: UpdateJob,
    /// Why the last status ask didn't go through.
    pub note: Option<String>,
    /// When the last job was asked for or ended: opening the screen probes only after a while.
    pub(crate) last: Option<Instant>,
}

impl FirmwareJob {
    /// Opening the screen doesn't probe again this soon after a firmware job.
    pub const QUIET: Duration = Duration::from_secs(60);

    /// The install running now, if one is.
    pub fn installing(&self) -> Option<crate::firmware::Unit> {
        use carpi_proto::FirmwareAction as A;
        if !self.job.running() {
            return None;
        }
        match self.action? {
            A::InstallRnse { .. } => Some(crate::firmware::Unit::Rnse),
            A::InstallTeensy { .. } => Some(crate::firmware::Unit::Teensy),
            A::Status | A::Probe => None,
        }
    }

    /// Running, or one ran within [`Self::QUIET`] of `now`.
    fn recent(&self, now: Instant) -> bool {
        self.job.running()
            || self
                .last
                .is_some_and(|t| now.saturating_duration_since(t) < Self::QUIET)
    }
}

/// The disks, as carpid last told them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Storage {
    /// `None` until carpid answers.
    pub status: Option<carpi_proto::StorageStatus>,
    /// Why the last ask didn't go through.
    pub note: Option<String>,
    /// The move, while it runs and how it ended.
    pub job: UpdateJob,
}

impl Storage {
    /// An NVMe is fitted and the system isn't on it: Move to NVMe is worth showing.
    pub fn can_move(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|s| s.nvme.is_some() && s.root_disk != carpi_proto::RootDisk::Nvme)
    }
}

/// Wi-Fi networks for updates, as carpid last told them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Wifi {
    /// `None` until carpid answers.
    pub networks: Option<Vec<carpi_proto::WifiNetwork>>,
    pub connected: Option<String>,
    /// Why the last list, add or forget didn't go through.
    pub note: Option<String>,
    /// The last scan or test.
    pub job: UpdateJob,
    /// What the last scan found.
    pub seen: Option<Vec<carpi_proto::WifiSeen>>,
}

/// An update job as it goes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateJob {
    /// Asked for, no job number yet.
    pub asked: bool,
    pub job: Option<u32>,
    pub stage: String,
    pub message: String,
    /// Done and total in the stage.
    pub progress: Option<(u64, Option<u64>)>,
    /// How it ended: ok, and why.
    pub finished: Option<(bool, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct UpdateAttempt {
    baseline: Option<carpi_proto::Installed>,
    target: Option<String>,
    applies: bool,
}

impl UpdateJob {
    pub fn running(&self) -> bool {
        (self.asked || self.job.is_some()) && self.finished.is_none()
    }

    fn event(&mut self, e: JobEvent) {
        match e {
            JobEvent::Stage { stage, message } => {
                self.stage = stage;
                self.message = message;
                self.progress = None;
            }
            JobEvent::Progress { done, total, .. } => self.progress = Some((done, total)),
            JobEvent::Finished { ok, message } => self.finished = Some((ok, message)),
            _ => {}
        }
    }

    fn refused(&mut self, why: String) {
        self.asked = false;
        self.finished = Some((false, why));
    }
}

impl Snapshot {
    /// carpid's firmware status replaces what the screen shows.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn take_firmware(&mut self, status: carpi_proto::FirmwareStatus) {
        self.firmware = status.into();
        self.firmware_job.note = None;
    }

    /// An event of the firmware job: its status replaces [`Snapshot::firmware`].
    #[cfg_attr(not(unix), allow(dead_code))]
    fn firmware_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Firmware(status) => self.take_firmware(*status),
            event => {
                if matches!(event, JobEvent::Finished { .. }) {
                    self.firmware_job.last = Some(Instant::now());
                }
                self.firmware_job.job.event(event);
            }
        }
    }

    /// carpid went away: what it said about the firmware no longer holds.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn firmware_disconnected(&mut self) {
        self.firmware = crate::firmware::Status::default();
        if self.firmware_job.job.running() {
            self.firmware_job.last = Some(Instant::now());
            self.firmware_job
                .job
                .refused("carpid went away; check versions again".into());
        }
    }

    fn start_update(&mut self, action: UpdateAction, and_apply: bool) {
        self.update_generation += 1;
        self.update_recovering = false;
        self.update_apply_observed = false;
        self.update_note = None;
        self.update_attempt = Some(UpdateAttempt {
            baseline: self.update_status.clone(),
            target: if action == UpdateAction::Apply {
                self.update_status
                    .as_ref()
                    .and_then(|s| s.downloaded.clone())
            } else {
                None
            },
            applies: action == UpdateAction::Apply || (action == UpdateAction::Fetch && and_apply),
        });
        self.update = UpdateJob {
            asked: true,
            ..Default::default()
        };
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    fn update_event(&mut self, event: JobEvent) {
        if let JobEvent::Stage { stage, .. } = &event {
            self.update_apply_observed |= matches!(
                stage.as_str(),
                "switch" | "restart" | "health" | "rollback" | "cleanup"
            );
        }
        self.update.event(event);
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    fn update_disconnected(&mut self) {
        if self.update.running() {
            self.update_apply_observed |= matches!(
                self.update.stage.as_str(),
                "switch" | "restart" | "health" | "rollback" | "cleanup"
            );
            self.update_recovering = true;
            self.update.asked = true;
            self.update.stage = "status".into();
            self.update.message = "Reconnecting to check the update result".into();
            self.update.progress = None;
        }
        // Job numbers belong to a connection's daemon; a restarted daemon can reuse them.
        self.update.job = None;
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    fn recover_update(&mut self, generation: u64, installed: carpi_proto::Installed) {
        if generation != self.update_generation || !self.update_recovering {
            return;
        }
        let running = installed.update_running == Some(true);
        // A live lock (or a busy status request) does not identify a job started by this UI.
        // Even an updater without the `applying` marker can report authoritative history.
        if !self.update.running() || self.update_attempt.is_none() {
            self.update_status = Some(installed);
            self.update_recovering = running;
            return;
        }
        self.update_apply_observed |=
            running && installed.last_apply.as_deref() == Some("applying");
        let correlated = self.update_apply_observed
            || self.update_attempt.as_ref().is_some_and(|attempt| {
                attempt.applies
                    && attempt.baseline.as_ref().is_some_and(|baseline| {
                        // A changed current release must match the known target or retain our
                        // baseline as previous. An unchanged old `ok` proves nothing about fetch.
                        (installed.current.is_some()
                            && installed.current != baseline.current
                            && ((attempt.target.is_some() && installed.current == attempt.target)
                                || (baseline.current.is_some()
                                    && installed.previous == baseline.current)))
                            || (installed.last_apply != baseline.last_apply
                                && matches!(
                                    installed.last_apply.as_deref(),
                                    Some("failed" | "rolled_back")
                                ))
                    })
            });
        let current = installed.current.as_deref().unwrap_or("unknown release");
        self.update_note = None;
        let finished = match installed.update_running {
            Some(true) => None,
            Some(false) if !correlated => {
                self.update_note = Some(format!(
                    "Update outcome unknown; check the update result; running {current}"
                ));
                None
            }
            Some(false) => match installed.last_apply.as_deref() {
                Some("ok") => Some((true, format!("{current} installed"))),
                Some("rolled_back") => {
                    Some((false, format!("Update rolled back; running {current}")))
                }
                Some("failed") => Some((false, format!("Update failed; running {current}"))),
                Some("applying") => {
                    Some((false, format!("Update was interrupted; running {current}")))
                }
                _ => {
                    self.update_note = Some(format!("Update outcome unknown; running {current}"));
                    None
                }
            },
            // Older daemons cannot establish that the previous outcome is this apply's result.
            None => {
                self.update_note =
                    Some("Update outcome unknown; updater status is unavailable".into());
                None
            }
        };
        self.update_status = Some(installed);
        self.update = UpdateJob {
            asked: running,
            stage: if running {
                "status".into()
            } else {
                String::new()
            },
            message: if running {
                "Waiting for the updater to finish".into()
            } else {
                String::new()
            },
            finished,
            ..Default::default()
        };
        self.update_recovering = running;
        if !running {
            self.update_apply_observed = false;
            self.update_attempt = None;
        }
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    fn update_status_failed(&mut self, generation: u64, message: String) {
        if self.update_recovering && generation == self.update_generation && self.update.running() {
            // Keep a local attempt pending, but never manufacture one from a passive probe.
            self.update = UpdateJob {
                asked: true,
                stage: "status".into(),
                message: format!("Could not check update result: {message}"),
                ..Default::default()
            };
        }
    }

    /// Notify on activation (including startup), and on deactivation.
    fn take_settings(&mut self, settings: Settings) {
        self.take_flashing_mode(settings.flashing_mode);
        self.settings = Some(settings);
    }

    fn take_flashing_mode(&mut self, on: bool) {
        if on && !self.flashing_mode {
            self.toast("Flashing mode ON: CAN sends blocked", ToastLevel::Warn);
        } else if !on && self.flashing_mode {
            self.toast("Flashing mode OFF", ToastLevel::Info);
        }
        self.flashing_mode = on;
        if let Some(settings) = self.settings.as_mut() {
            settings.flashing_mode = on;
        }
    }

    fn take_values(&mut self, items: Vec<Reading>, now: Instant) {
        if let Some(on) = items.iter().find_map(|r| match (&*r.name, &r.value) {
            ("system.flashing_mode", Some(carpi_proto::Value::Bool(on))) => Some(*on),
            _ => None,
        }) {
            self.take_flashing_mode(on);
        }
        self.values.take(items, now);
    }

    pub fn toast(&mut self, text: impl Into<String>, level: ToastLevel) {
        let seq = self.toast.as_ref().map_or(1, |t| t.seq + 1);
        self.toast = Some(Toast {
            seq,
            text: text.into(),
            level,
        });
    }

    /// A capture that came back, or didn't.
    fn captured(&mut self, result: Result<(String, String), String>) {
        match result {
            Ok((label, summary)) => {
                self.toast(format!("Capture {label}"), ToastLevel::Info);
                self.capture = Capture::Saved { label, summary };
            }
            Err(why) => {
                self.toast(format!("Capture failed: {why}"), ToastLevel::Error);
                self.capture = Capture::Failed(why);
            }
        }
    }
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            flashing_mode: false,
            connected: false,
            daemon: None,
            problem: Some("connecting to carpid".into()),
            values: Store::default(),
            buses: None,
            capture: Capture::Idle,
            diag: Diag::default(),
            settings: None,
            restart_needed: Vec::new(),
            settings_note: None,
            toast: None,
            datalog: None,
            datalog_note: None,
            update: UpdateJob::default(),
            update_status: None,
            update_recovering: true,
            update_generation: 0,
            update_apply_observed: false,
            update_attempt: None,
            update_note: None,
            wifi: Wifi::default(),
            storage: Storage::default(),
            decoder: UpdateJob::default(),
            firmware: crate::firmware::Status::default(),
            firmware_job: FirmwareJob::default(),
        }
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
enum Ask {
    Shown(bool),
    Capture,
    /// `None` asks for the settings, a patch changes them.
    Settings(Option<Box<SettingsPatch>>),
    Light(LightAction),
    Diag(DiagAction),
    Datalog(DatalogAction),
    Update(UpdateAction, bool),
    UpdateStatus,
    Wifi(carpi_proto::WifiAction),
    Storage(carpi_proto::StorageAction),
    DecoderCrc,
    Firmware(carpi_proto::FirmwareAction),
    /// The names to watch changed.
    Watch,
}

#[derive(Clone)]
pub struct Client {
    shared: Arc<Mutex<Snapshot>>,
    asks: Option<Sender<Ev>>,
    /// Diag catalog names subscribed while a page is shown.
    watch: Arc<Mutex<Vec<String>>>,
}

impl Client {
    /// A client that never connects.
    pub fn offline(why: &str) -> Self {
        let snap = Snapshot {
            problem: Some(why.into()),
            ..Snapshot::default()
        };
        Self {
            shared: Arc::new(Mutex::new(snap)),
            asks: None,
            watch: Arc::default(),
        }
    }

    /// Connects to carpid, or reads `CARPI_UI_FIXTURE` when it's set.
    pub fn start() -> Self {
        if let Some(path) = std::env::var_os("CARPI_UI_FIXTURE") {
            return match Fixture::read(path.as_ref()) {
                Ok(f) => f.start(),
                Err(e) => Self::offline(&format!("fixture: {e}")),
            };
        }
        #[cfg(unix)]
        return Self::connect(link::socket());
        #[cfg(not(unix))]
        Self::offline("carpid needs Unix sockets")
    }

    /// Connects to carpid at `socket`, and again whenever the connection drops.
    #[cfg(unix)]
    pub fn connect(socket: std::path::PathBuf) -> Self {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let watch = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let inner = shared.clone();
        let names = watch.clone();
        let ev = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("carpi-ui".into())
            .spawn(move || link::run(&socket, inner, names, ev, rx));
        match spawned {
            Ok(_) => Self {
                shared,
                asks: Some(tx),
                watch,
            },
            Err(e) => Self::offline(&format!("no thread: {e}")),
        }
    }

    pub fn snapshot(&self) -> MutexGuard<'_, Snapshot> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One of our pages is on screen, or none is.
    pub fn shown(&self, on: bool) {
        self.ask(Ask::Shown(on));
    }

    /// Asks carpid for a capture, as the page's trigger; the answer comes back as a toast.
    pub fn capture(&self) {
        if self.asks.is_none() {
            self.snapshot().captured(Err("not connected".into()));
        }
        self.ask(Ask::Capture);
    }

    /// Asks carpid for its settings again.
    pub fn settings(&self) {
        self.ask(Ask::Settings(None));
    }

    /// Changes carpid's settings; the reply replaces [`Snapshot::settings`].
    pub fn set(&self, patch: SettingsPatch) {
        if self.asks.is_none() {
            self.snapshot().settings_note = Some("not connected".into());
        }
        self.ask(Ask::Settings(Some(Box::new(patch))));
    }

    /// A brightness nudge or "reset learned" for the light model.
    pub fn light(&self, action: LightAction) {
        if self.asks.is_none() {
            self.snapshot().settings_note = Some("not connected".into());
        }
        self.ask(Ask::Light(action));
    }

    /// Starts a diag job; its events land in [`Snapshot::diag`].
    pub fn diag(&self, action: DiagAction) {
        if self.asks.is_none() {
            self.snapshot().diag.refused(action, "not connected".into());
        }
        self.ask(Ask::Diag(action));
    }

    /// A data-log ask; carpid's answer replaces [`Snapshot::datalog`].
    pub fn datalog(&self, action: DatalogAction) {
        if self.asks.is_none() {
            self.snapshot().datalog_note = Some("not connected".into());
        }
        self.ask(Ask::Datalog(action));
    }

    /// A Wi-Fi network ask: list, add and forget answer in [`Snapshot::wifi`]; scan and test
    /// are jobs whose events land in its `job`.
    pub fn wifi(&self, action: carpi_proto::WifiAction) {
        use carpi_proto::WifiAction as W;
        {
            let mut s = self.snapshot();
            s.wifi.note = None;
            if matches!(action, W::Scan | W::Test { .. }) {
                s.wifi.job = UpdateJob {
                    asked: true,
                    ..Default::default()
                };
                if matches!(action, W::Scan) {
                    s.wifi.seen = None;
                }
                if self.asks.is_none() {
                    s.wifi.job.refused("not connected".into());
                }
            } else if self.asks.is_none() {
                s.wifi.note = Some("not connected".into());
            }
        }
        self.ask(Ask::Wifi(action));
    }

    /// Asks where the system runs from, or starts Move to NVMe; answers and the move's events
    /// land in [`Snapshot::storage`].
    pub fn storage(&self, action: carpi_proto::StorageAction) {
        {
            let mut s = self.snapshot();
            s.storage.note = None;
            if action == carpi_proto::StorageAction::MoveToNvme {
                s.storage.job = UpdateJob {
                    asked: true,
                    ..Default::default()
                };
                if self.asks.is_none() {
                    s.storage.job.refused("not connected".into());
                }
            } else if self.asks.is_none() {
                s.storage.note = Some("not connected".into());
            }
        }
        self.ask(Ask::Storage(action));
    }

    pub fn decoder_crc(&self) {
        {
            let mut s = self.snapshot();
            if s.decoder.running() {
                return;
            }
            s.decoder = UpdateJob {
                asked: true,
                ..Default::default()
            };
            if self.asks.is_none() {
                s.decoder.refused("not connected".into());
            }
        }
        self.ask(Ask::DecoderCrc);
    }

    /// A firmware ask: `Status` answers in [`Snapshot::firmware`]; probe and installs are jobs
    /// whose events land in [`Snapshot::firmware_job`], one at a time.
    pub fn firmware(&self, action: carpi_proto::FirmwareAction) {
        {
            let mut s = self.snapshot();
            if action == carpi_proto::FirmwareAction::Status {
                s.firmware_job.note = None;
                if self.asks.is_none() {
                    s.firmware_job.note = Some("not connected".into());
                }
            } else {
                if s.firmware_job.job.running() {
                    return;
                }
                s.firmware_job = FirmwareJob {
                    action: Some(action),
                    job: UpdateJob {
                        asked: true,
                        ..Default::default()
                    },
                    note: None,
                    last: Some(Instant::now()),
                };
                if self.asks.is_none() {
                    s.firmware_job.job.refused("not connected".into());
                }
            }
        }
        self.ask(Ask::Firmware(action));
    }

    /// The firmware screen opened: carpid's status now, and a fresh probe unless a firmware
    /// job is running or ran in the last minute.
    pub fn firmware_opened(&self) {
        self.firmware(carpi_proto::FirmwareAction::Status);
        let probe = {
            let s = self.snapshot();
            s.connected && !s.firmware_job.recent(Instant::now())
        };
        if probe {
            self.firmware(carpi_proto::FirmwareAction::Probe);
        }
    }

    /// Starts an update job; its events land in [`Snapshot::update`].
    pub fn update(&self, action: UpdateAction, and_apply: bool) {
        if action == UpdateAction::Status {
            self.update_status();
            return;
        }
        {
            let mut s = self.snapshot();
            s.start_update(action, and_apply);
            if self.asks.is_none() {
                s.update.refused("not connected".into());
            }
        }
        self.ask(Ask::Update(action, and_apply));
    }

    /// Refresh the persisted result without replacing a live update job.
    pub fn update_status(&self) {
        self.ask(Ask::UpdateStatus);
    }

    /// Diag catalog names to keep subscribed while a page is shown; carpid polls them for as
    /// long as they are. Asks carpid only when they change.
    pub fn watch(&self, names: Vec<String>) {
        {
            let mut w = self.watch.lock().unwrap_or_else(|e| e.into_inner());
            if *w == names {
                return;
            }
            *w = names;
        }
        self.ask(Ask::Watch);
    }

    /// A note the pages show with the settings.
    pub fn note(&self, note: impl Into<String>) {
        self.snapshot().settings_note = Some(note.into());
    }

    fn ask(&self, ask: Ask) {
        if let Some(tx) = &self.asks {
            let _ = tx.send(Ev::Ask(ask));
        }
    }
}

/// The link thread takes asks and carpid's replies on one channel.
enum Ev {
    Ask(Ask),
    #[cfg_attr(not(unix), allow(dead_code))]
    Got(u64, Box<carpi_proto::Response>),
    #[cfg_attr(not(unix), allow(dead_code))]
    Gone(u64, String),
}

#[cfg(unix)]
mod link {
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};

    use carpi_proto::{PROTOCOL_VERSION, Request, Response};

    use super::*;

    const POLL: Duration = Duration::from_secs(1);
    const PERIOD_MS: u32 = 100;
    const SUB: u32 = 1;
    const RETRY_FIRST: Duration = Duration::from_secs(1);
    const RETRY_MAX: Duration = Duration::from_secs(10);
    /// Requests in flight before polling waits for carpid.
    const IN_FLIGHT: usize = 6;

    pub(super) fn socket() -> PathBuf {
        if let Some(path) = std::env::var_os("CARPID_SOCKET") {
            return path.into();
        }
        let dir = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
        PathBuf::from(dir).join("carpid.sock")
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Pending {
        Hello,
        Values,
        Bus,
        Capture,
        Settings,
        Light,
        Diag(DiagAction),
        Datalog,
        Update,
        UpdateStatus(u64),
        /// List, add, forget.
        Wifi,
        /// Scan, test.
        WifiJob,
        Storage,
        /// Move to NVMe.
        StorageJob,
        DecoderCrc,
        /// Firmware status.
        Firmware,
        /// Probe, installs.
        FirmwareJob,
        Catalog,
        Ok,
    }

    /// The diag catalog names the diagnostics page shows.
    const WATCH: u32 = 2;
    const WATCH_MS: u32 = 500;

    pub(super) fn run(
        socket: &Path,
        shared: Arc<Mutex<Snapshot>>,
        watch: Arc<Mutex<Vec<String>>>,
        tx: Sender<Ev>,
        rx: Receiver<Ev>,
    ) {
        let mut shown = false;
        let mut retry = RETRY_FIRST;
        let mut id = 0u64;
        loop {
            id += 1;
            let why = match UnixStream::connect(socket) {
                Ok(stream) => {
                    retry = RETRY_FIRST;
                    session(id, stream, &shared, &watch, &tx, &rx, &mut shown)
                }
                Err(e) => format!("carpid not running ({e})"),
            };
            {
                let mut s = lock(&shared);
                s.connected = false;
                s.problem = Some(why);
                s.update_disconnected();
                if s.decoder.running() {
                    s.decoder.refused("carpid went away; read again".into());
                }
                s.firmware_disconnected();
                if s.capture == Capture::Saving {
                    s.captured(Err("carpid went away".into()));
                }
                if let Some(crate::diag::Job::Running { action, .. }) = s.diag.job {
                    s.diag.refused(action, "carpid went away".into());
                }
            }
            let until = Instant::now() + retry;
            retry = (retry * 2).min(RETRY_MAX);
            // Keep taking asks while waiting.
            while let Some(left) = until.checked_duration_since(Instant::now()) {
                match rx.recv_timeout(left) {
                    Ok(Ev::Ask(Ask::Shown(on))) => shown = on,
                    Ok(Ev::Ask(Ask::Capture)) => {
                        lock(&shared).captured(Err("not connected".into()))
                    }
                    Ok(Ev::Ask(Ask::Settings(Some(_)) | Ask::Light(_))) => {
                        lock(&shared).settings_note = Some("not connected".into())
                    }
                    Ok(Ev::Ask(Ask::Diag(action))) => {
                        lock(&shared).diag.refused(action, "not connected".into())
                    }
                    Ok(Ev::Ask(Ask::DecoderCrc)) => {
                        lock(&shared).decoder.refused("not connected".into())
                    }
                    Ok(Ev::Ask(Ask::Firmware(carpi_proto::FirmwareAction::Status))) => {
                        lock(&shared).firmware_job.note = Some("not connected".into())
                    }
                    Ok(Ev::Ask(Ask::Firmware(_))) => lock(&shared)
                        .firmware_job
                        .job
                        .refused("not connected".into()),
                    Ok(Ev::Ask(Ask::Datalog(_))) => {
                        lock(&shared).datalog_note = Some("not connected".into())
                    }
                    Ok(Ev::Ask(Ask::Update(..))) => {
                        lock(&shared).update.refused("not connected".into())
                    }
                    Ok(Ev::Ask(Ask::Storage(carpi_proto::StorageAction::MoveToNvme))) => {
                        lock(&shared).storage.job.refused("not connected".into())
                    }
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }

    fn lock(shared: &Mutex<Snapshot>) -> MutexGuard<'_, Snapshot> {
        shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read(stream: &mut UnixStream) -> io::Result<Response> {
        let mut prefix = [0u8; 4];
        stream.read_exact(&mut prefix)?;
        let n = carpi_proto::body_len(prefix).map_err(io::Error::other)?;
        let mut body = vec![0u8; n];
        stream.read_exact(&mut body)?;
        carpi_proto::decode(&body).map_err(io::Error::other)
    }

    /// Runs one connection until it fails; returns why.
    fn session(
        id: u64,
        stream: UnixStream,
        shared: &Mutex<Snapshot>,
        watch: &Arc<Mutex<Vec<String>>>,
        tx: &Sender<Ev>,
        rx: &Receiver<Ev>,
        shown: &mut bool,
    ) -> String {
        let mut reader = match stream.try_clone() {
            Ok(r) => r,
            Err(e) => return format!("socket: {e}"),
        };
        let back = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("carpi-ui-read".into())
            .spawn(move || {
                loop {
                    match read(&mut reader) {
                        Ok(resp) => {
                            if back.send(Ev::Got(id, Box::new(resp))).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = back.send(Ev::Gone(id, format!("carpid closed ({e})")));
                            return;
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            return format!("no thread: {e}");
        }
        let mut link = Link {
            stream,
            pending: VecDeque::new(),
            watch: watch.clone(),
            watching: false,
            update_status: None,
        };
        let why = link.drive(id, shared, rx, shown);
        let _ = link.stream.shutdown(std::net::Shutdown::Both);
        why
    }

    struct Link {
        stream: UnixStream,
        pending: VecDeque<Pending>,
        watch: Arc<Mutex<Vec<String>>>,
        /// The watch subscription is on.
        watching: bool,
        update_status: Option<StatusJob>,
    }

    struct StatusJob {
        job: u32,
        generation: u64,
        installed: Option<carpi_proto::Installed>,
    }

    impl Link {
        fn poll_update_status(
            &mut self,
            shared: &Mutex<Snapshot>,
            refresh: bool,
        ) -> io::Result<()> {
            let mut s = lock(shared);
            if refresh && !s.update.running() {
                s.update_recovering = true;
            }
            if !s.update_recovering
                || self.update_status.is_some()
                || self
                    .pending
                    .iter()
                    .any(|p| matches!(p, Pending::UpdateStatus(_)))
            {
                return Ok(());
            }
            let generation = s.update_generation;
            drop(s);
            self.send(
                &Request::Update {
                    action: UpdateAction::Status,
                    and_apply: false,
                },
                Pending::UpdateStatus(generation),
            )
        }

        fn send(&mut self, req: &Request, pending: Pending) -> io::Result<()> {
            self.stream.write_all(&carpi_proto::encode(req))?;
            self.pending.push_back(pending);
            Ok(())
        }

        fn poll(&mut self) -> io::Result<()> {
            if self.pending.len() > IN_FLIGHT {
                return Ok(());
            }
            self.send(&Request::Values { prefix: "".into() }, Pending::Values)?;
            self.send(&Request::Bus, Pending::Bus)?;
            self.datalog(DatalogAction::Status)
        }

        fn datalog(&mut self, action: DatalogAction) -> io::Result<()> {
            self.send(&Request::Datalog { action }, Pending::Datalog)
        }

        fn subscribe(&mut self, on: bool) -> io::Result<()> {
            if on {
                let req = Request::Subscribe {
                    sub: SUB,
                    prefixes: vec!["".into()],
                    period_ms: PERIOD_MS,
                };
                self.send(&req, Pending::Ok)?;
                self.settings(None)?;
                self.rewatch()?;
                self.poll()
            } else {
                self.send(&Request::Unsubscribe { sub: SUB }, Pending::Ok)?;
                self.unwatch()
            }
        }

        fn unwatch(&mut self) -> io::Result<()> {
            if std::mem::take(&mut self.watching) {
                self.send(&Request::Unsubscribe { sub: WATCH }, Pending::Ok)?;
            }
            Ok(())
        }

        /// The watch subscription again, for the names now wanted.
        fn rewatch(&mut self) -> io::Result<()> {
            self.unwatch()?;
            let names = self.watch.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if names.is_empty() {
                return Ok(());
            }
            let req = Request::Subscribe {
                sub: WATCH,
                prefixes: names,
                period_ms: WATCH_MS,
            };
            self.send(&req, Pending::Ok)?;
            self.watching = true;
            Ok(())
        }

        fn settings(&mut self, patch: Option<Box<SettingsPatch>>) -> io::Result<()> {
            let action = match patch {
                None => carpi_proto::SettingsAction::Get,
                Some(patch) => carpi_proto::SettingsAction::Set { patch },
            };
            self.send(&Request::Settings { action }, Pending::Settings)
        }

        fn poll_flashing_mode(&mut self) -> io::Result<()> {
            if self.pending.len() > IN_FLIGHT {
                return Ok(());
            }
            self.send(
                &Request::Values {
                    prefix: "system.flashing_mode".into(),
                },
                Pending::Values,
            )
        }

        fn drive(
            &mut self,
            id: u64,
            shared: &Mutex<Snapshot>,
            rx: &Receiver<Ev>,
            shown: &mut bool,
        ) -> String {
            let hello = Request::Hello {
                version: PROTOCOL_VERSION,
                client: "livi-carpi-ui".into(),
            };
            if let Err(e) = self.send(&hello, Pending::Hello) {
                return format!("carpid: {e}");
            }
            if *shown && let Err(e) = self.subscribe(true) {
                return format!("carpid: {e}");
            }
            let mut next = Instant::now() + POLL;
            loop {
                let wait = next.saturating_duration_since(Instant::now());
                let sent = match rx.recv_timeout(wait) {
                    Err(RecvTimeoutError::Timeout) => {
                        next = Instant::now() + POLL;
                        let status = self.poll_update_status(shared, false);
                        status.and_then(|()| {
                            if *shown {
                                self.poll()
                            } else {
                                self.poll_flashing_mode()
                            }
                        })
                    }
                    Err(RecvTimeoutError::Disconnected) => return "closed".into(),
                    Ok(Ev::Ask(Ask::Shown(on))) => {
                        let was = std::mem::replace(shown, on);
                        if was != on {
                            self.subscribe(on)
                        } else {
                            Ok(())
                        }
                    }
                    Ok(Ev::Ask(Ask::Capture)) => {
                        lock(shared).capture = Capture::Saving;
                        let req = Request::Capture {
                            trigger: "page".into(),
                        };
                        self.send(&req, Pending::Capture)
                    }
                    Ok(Ev::Ask(Ask::Settings(patch))) => self.settings(patch),
                    Ok(Ev::Ask(Ask::Light(action))) => {
                        self.send(&Request::Light { action }, Pending::Light)
                    }
                    Ok(Ev::Ask(Ask::Diag(action))) => {
                        self.send(&Request::Diag { action }, Pending::Diag(action))
                    }
                    Ok(Ev::Ask(Ask::Datalog(action))) => self.datalog(action),
                    Ok(Ev::Ask(Ask::Update(action, and_apply))) => {
                        self.send(&Request::Update { action, and_apply }, Pending::Update)
                    }
                    Ok(Ev::Ask(Ask::UpdateStatus)) => self.poll_update_status(shared, true),
                    Ok(Ev::Ask(Ask::Wifi(action))) => {
                        let job = matches!(
                            action,
                            carpi_proto::WifiAction::Scan | carpi_proto::WifiAction::Test { .. }
                        );
                        self.send(
                            &Request::Wifi { action },
                            if job { Pending::WifiJob } else { Pending::Wifi },
                        )
                    }
                    Ok(Ev::Ask(Ask::Storage(action))) => {
                        let pending = match action {
                            carpi_proto::StorageAction::Status => Pending::Storage,
                            carpi_proto::StorageAction::MoveToNvme => Pending::StorageJob,
                        };
                        self.send(&Request::Storage { action }, pending)
                    }
                    Ok(Ev::Ask(Ask::DecoderCrc)) => {
                        self.send(&Request::DecoderCrc, Pending::DecoderCrc)
                    }
                    Ok(Ev::Ask(Ask::Firmware(action))) => {
                        let pending = if action == carpi_proto::FirmwareAction::Status {
                            Pending::Firmware
                        } else {
                            Pending::FirmwareJob
                        };
                        self.send(&Request::Firmware { action }, pending)
                    }
                    Ok(Ev::Ask(Ask::Watch)) if *shown => self.rewatch(),
                    Ok(Ev::Ask(Ask::Watch)) => Ok(()),
                    Ok(Ev::Got(from, resp)) if from == id => {
                        if let Err(why) = self.got(shared, *resp) {
                            return why;
                        }
                        Ok(())
                    }
                    Ok(Ev::Gone(from, why)) if from == id => return why,
                    Ok(_) => Ok(()),
                };
                if let Err(e) = sent {
                    return format!("carpid: {e}");
                }
            }
        }

        fn got(&mut self, shared: &Mutex<Snapshot>, resp: Response) -> Result<(), String> {
            let now = Instant::now();
            let mut s = lock(shared);
            match resp {
                Response::Update { items, .. } => {
                    s.take_values(items, now);
                    return Ok(());
                }
                Response::Job { job, event } => {
                    if self
                        .update_status
                        .as_ref()
                        .is_some_and(|status| status.job == job)
                    {
                        match event {
                            JobEvent::Installed(installed) => {
                                self.update_status.as_mut().unwrap().installed = Some(installed);
                            }
                            JobEvent::Finished { ok, message } => {
                                let status = self.update_status.take().unwrap();
                                if ok && let Some(installed) = status.installed {
                                    s.recover_update(status.generation, installed);
                                } else {
                                    s.update_status_failed(status.generation, message);
                                }
                            }
                            _ => {}
                        }
                    } else if s.wifi.job.job == Some(job) {
                        if let JobEvent::WifiSeen { networks } = &event {
                            s.wifi.seen = Some(networks.clone());
                        }
                        s.wifi.job.event(event);
                    } else if s.update.job == Some(job) {
                        s.update_event(event);
                    } else if s.storage.job.job == Some(job) {
                        s.storage.job.event(event);
                    } else if s.decoder.job == Some(job) {
                        s.decoder.event(event);
                    } else if s.firmware_job.job.job == Some(job) {
                        s.firmware_event(event);
                    } else {
                        s.diag.event(job, event);
                    }
                    return Ok(());
                }
                // Not ours: only the bridge asks for commands.
                Response::Command { .. } => return Ok(()),
                _ => {}
            }
            let pending = self.pending.pop_front();
            match (pending, resp) {
                (Some(Pending::Hello), Response::Hello { version, daemon }) => {
                    if version != carpi_proto::PROTOCOL_VERSION {
                        return Err(format!(
                            "carpid speaks protocol {version}, the pages {}",
                            carpi_proto::PROTOCOL_VERSION
                        ));
                    }
                    s.connected = true;
                    s.daemon = Some(daemon);
                    s.problem = None;
                    drop(s);
                    // Once a connection: the pickers' catalog.
                    self.send(&Request::Catalog, Pending::Catalog)
                        .map_err(|e| format!("carpid: {e}"))?;
                    self.poll_update_status(shared, true)
                        .map_err(|e| format!("carpid: {e}"))?;
                    return Ok(());
                }
                (_, Response::Catalog { categories, values }) => {
                    crate::catalog::set(categories, values);
                }
                // An old carpid: the built-in table stays.
                (Some(Pending::Catalog), Response::Error { .. }) => {}
                (Some(Pending::Hello), Response::Error { message }) => {
                    return Err(format!("carpid refused: {message}"));
                }
                (_, Response::Values { items }) => s.take_values(items, now),
                (_, Response::Bus { buses }) => s.buses = Some(buses),
                (_, Response::Captured { label, summary, .. }) => s.captured(Ok((label, summary))),
                (Some(Pending::Capture), Response::Error { message }) => s.captured(Err(message)),
                (
                    _,
                    Response::Settings {
                        settings,
                        restart_needed,
                    },
                ) => {
                    s.take_settings(settings);
                    s.restart_needed = restart_needed;
                    s.settings_note = None;
                }
                (Some(Pending::Settings | Pending::Light), Response::Error { message }) => {
                    s.settings_note = Some(message)
                }
                (Some(Pending::Update), Response::JobStarted { job }) => {
                    s.update.asked = false;
                    s.update.job = Some(job);
                }
                (Some(Pending::Update), Response::Error { message }) => s.update.refused(message),
                (Some(Pending::UpdateStatus(generation)), Response::JobStarted { job }) => {
                    self.update_status = Some(StatusJob {
                        job,
                        generation,
                        installed: None,
                    });
                }
                (Some(Pending::UpdateStatus(generation)), Response::Error { message }) => {
                    s.update_status_failed(generation, message);
                }
                (
                    _,
                    Response::Wifi {
                        networks,
                        connected,
                    },
                ) => {
                    s.wifi.networks = Some(networks);
                    s.wifi.connected = connected;
                    s.wifi.note = None;
                }
                (Some(Pending::Wifi), Response::Error { message }) => s.wifi.note = Some(message),
                (Some(Pending::WifiJob), Response::JobStarted { job }) => {
                    s.wifi.job.asked = false;
                    s.wifi.job.job = Some(job);
                }
                (Some(Pending::WifiJob), Response::Error { message }) => {
                    s.wifi.job.refused(message)
                }
                (_, Response::Storage { storage }) => {
                    s.storage.status = Some(storage);
                    s.storage.note = None;
                }
                (Some(Pending::Storage), Response::Error { message }) => {
                    s.storage.note = Some(message)
                }
                (Some(Pending::StorageJob), Response::JobStarted { job }) => {
                    s.storage.job.asked = false;
                    s.storage.job.job = Some(job);
                }
                (Some(Pending::StorageJob), Response::Error { message }) => {
                    s.storage.job.refused(message)
                }
                (Some(Pending::DecoderCrc), Response::JobStarted { job }) => {
                    s.decoder.asked = false;
                    s.decoder.job = Some(job);
                }
                (Some(Pending::DecoderCrc), Response::Error { message }) => {
                    s.decoder.refused(message)
                }
                (_, Response::Firmware { status }) => s.take_firmware(status),
                (Some(Pending::Firmware), Response::Error { message }) => {
                    s.firmware_job.note = Some(message)
                }
                (Some(Pending::FirmwareJob), Response::JobStarted { job }) => {
                    s.firmware_job.job.asked = false;
                    s.firmware_job.job.job = Some(job);
                }
                (Some(Pending::FirmwareJob), Response::Error { message }) => {
                    s.firmware_job.last = Some(Instant::now());
                    s.firmware_job.job.refused(message)
                }
                (Some(Pending::Diag(action)), Response::JobStarted { job }) => {
                    s.diag.started(job, action)
                }
                (Some(Pending::Diag(action)), Response::Error { message }) => {
                    s.diag.refused(action, message)
                }
                (_, Response::Datalog { status }) => {
                    s.datalog = Some(*status);
                    s.datalog_note = None;
                }
                (Some(Pending::Datalog), Response::Error { message }) => {
                    s.datalog_note = Some(message)
                }
                (_, Response::Error { message }) => s.problem = Some(message),
                _ => {}
            }
            Ok(())
        }
    }
}

/// A fixed set of readings for snapshots and tests:
/// `{"values": [Reading], "buses": [BusInfo], "settings": Settings}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Fixture {
    pub daemon: Option<String>,
    pub values: Vec<Reading>,
    pub buses: Option<Vec<BusInfo>>,
    pub settings: Option<Settings>,
    pub datalog: Option<DatalogStatus>,
    pub storage: Option<carpi_proto::StorageStatus>,
    pub firmware: crate::firmware::Status,
}

impl Fixture {
    pub fn read(path: &std::path::Path) -> Result<Self, String> {
        let body = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_slice(&body).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Fills a snapshot now and every second, so nothing goes stale.
    pub fn start(self) -> Client {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        {
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            self.fill(&mut s);
            // Fixtures have no updater status probe to finish.
            s.update_recovering = false;
            if let Some(settings) = self.settings.clone() {
                s.take_settings(settings);
            }
            s.datalog = self.datalog.clone();
            s.storage.status = self.storage.clone();
        }
        let (tx, rx) = mpsc::channel();
        let inner = shared.clone();
        let _ = std::thread::Builder::new()
            .name("carpi-ui-fixture".into())
            .spawn(move || self.run(&inner, &rx));
        Client {
            shared,
            asks: Some(tx),
            watch: Arc::default(),
        }
    }

    fn fill(&self, s: &mut Snapshot) {
        s.connected = true;
        s.problem = None;
        s.daemon = Some(self.daemon.clone().unwrap_or_else(|| "fixture".into()));
        s.take_values(self.values.clone(), Instant::now());
        s.buses = self.buses.clone();
        s.firmware = self.firmware.clone();
    }

    fn run(&self, shared: &Mutex<Snapshot>, rx: &Receiver<Ev>) {
        loop {
            let ask = rx.recv_timeout(Duration::from_secs(1));
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            match ask {
                Ok(Ev::Ask(Ask::Capture)) => {
                    s.captured(Ok(("2026-11-08 #01".into(), "fixture".into())))
                }
                Ok(Ev::Ask(Ask::Settings(Some(patch)))) => {
                    if let Some(mut settings) = s.settings.clone() {
                        crate::settings::apply(&mut settings, &patch);
                        s.take_settings(settings);
                    }
                }
                Ok(Ev::Ask(Ask::Datalog(action))) => {
                    if let Some(st) = &mut s.datalog {
                        crate::logging::apply(st, &action);
                    }
                }
                Ok(Ev::Ask(Ask::Update(..))) => {
                    s.update.asked = false;
                    s.update.job = Some(1);
                    s.update.event(JobEvent::Stage {
                        stage: "download".into(),
                        message: "Downloading".into(),
                    });
                    s.update.event(JobEvent::Progress {
                        stage: "download".into(),
                        done: 12 << 20,
                        total: Some(40 << 20),
                    });
                }
                Ok(Ev::Ask(Ask::Storage(carpi_proto::StorageAction::MoveToNvme))) => {
                    s.storage.job.asked = false;
                    s.storage.job.job = Some(2);
                    s.storage.job.event(JobEvent::Stage {
                        stage: "copy".into(),
                        message: "Copying the system".into(),
                    });
                    s.storage.job.event(JobEvent::Progress {
                        stage: "copy".into(),
                        done: 37,
                        total: Some(100),
                    });
                }
                Ok(Ev::Ask(Ask::DecoderCrc)) => {
                    s.decoder
                        .refused("decoder reads need a live carpid connection".into());
                }
                Ok(Ev::Ask(Ask::Firmware(carpi_proto::FirmwareAction::Status))) => {}
                Ok(Ev::Ask(Ask::Firmware(_))) => {
                    s.firmware_job.last = Some(Instant::now());
                    s.firmware_job
                        .job
                        .refused("firmware jobs need a live carpid connection".into());
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.fill(&mut s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_action_queues_one_read_and_offline_read_reports_error() {
        let (tx, rx) = mpsc::channel();
        let client = Client {
            shared: Arc::default(),
            asks: Some(tx),
            watch: Arc::default(),
        };
        client.decoder_crc();
        assert!(client.snapshot().decoder.running());
        assert!(matches!(rx.try_recv(), Ok(Ev::Ask(Ask::DecoderCrc))));
        client.decoder_crc();
        assert!(
            rx.try_recv().is_err(),
            "must not start a second overlapping read"
        );
        let offline = Client::offline("offline");
        offline.decoder_crc();
        assert_eq!(
            offline.snapshot().decoder.finished,
            Some((false, "not connected".into()))
        );
        let request = carpi_proto::encode(&carpi_proto::Request::DecoderCrc);
        assert_eq!(
            carpi_proto::decode::<carpi_proto::Request>(&request[4..]).unwrap(),
            carpi_proto::Request::DecoderCrc
        );
    }

    fn asking_client() -> (Client, Receiver<Ev>) {
        let (tx, rx) = mpsc::channel();
        let client = Client {
            shared: Arc::default(),
            asks: Some(tx),
            watch: Arc::default(),
        };
        (client, rx)
    }

    #[test]
    fn firmware_jobs_run_one_at_a_time_and_offline_jobs_are_refused() {
        use carpi_proto::FirmwareAction as A;
        let (client, rx) = asking_client();
        client.firmware(A::InstallRnse { confirm: true });
        assert!(client.snapshot().firmware_job.job.running());
        assert_eq!(
            client.snapshot().firmware_job.installing(),
            Some(crate::firmware::Unit::Rnse)
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Firmware(A::InstallRnse { confirm: true })))
        ));
        client.firmware(A::Probe);
        assert!(rx.try_recv().is_err(), "no second firmware job");
        // Status still goes while a job runs.
        client.firmware(A::Status);
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Firmware(A::Status)))
        ));

        let offline = Client::offline("offline");
        offline.firmware(A::Probe);
        assert_eq!(
            offline.snapshot().firmware_job.job.finished,
            Some((false, "not connected".into()))
        );
        let request = carpi_proto::encode(&carpi_proto::Request::Firmware {
            action: A::InstallTeensy { confirm: true },
        });
        assert_eq!(
            carpi_proto::decode::<carpi_proto::Request>(&request[4..]).unwrap(),
            carpi_proto::Request::Firmware {
                action: A::InstallTeensy { confirm: true }
            }
        );
    }

    #[test]
    fn firmware_status_is_replaced_by_replies_and_job_events() {
        use crate::firmware::{Comparison, Installed};
        let mut s = Snapshot::default();
        let mut status = carpi_proto::FirmwareStatus {
            busy: true,
            ..Default::default()
        };
        s.take_firmware(status.clone());
        assert!(s.firmware.busy);
        s.firmware_job.job = UpdateJob {
            job: Some(9),
            ..Default::default()
        };
        s.firmware_event(JobEvent::Stage {
            stage: "read".into(),
            message: "reading the module header".into(),
        });
        status.busy = false;
        status.rnse.installed = carpi_proto::FirmwareInstalled::Legacy;
        status.rnse.comparison = carpi_proto::FirmwareComparison::Newer;
        s.firmware_event(JobEvent::Firmware(Box::new(status.clone())));
        assert!(!s.firmware.busy);
        assert_eq!(s.firmware.rnse.installed, Installed::Legacy);
        assert_eq!(s.firmware.rnse.comparison, Comparison::Newer);
        assert!(
            s.firmware_job.job.running(),
            "the status event doesn't end the job"
        );
        assert_eq!(s.firmware_job.job.message, "reading the module header");
        // A later event replaces the whole status, not just some fields.
        s.firmware_event(JobEvent::Firmware(Box::default()));
        assert_eq!(s.firmware, crate::firmware::Status::default());
        s.firmware_event(JobEvent::Finished {
            ok: true,
            message: "versions read".into(),
        });
        assert!(!s.firmware_job.job.running());
        assert!(s.firmware_job.last.is_some());
        // carpid going away clears the status and ends a running job.
        s.take_firmware(status);
        s.firmware_job.job.finished = None;
        s.firmware_disconnected();
        assert_eq!(s.firmware, crate::firmware::Status::default());
        assert!(!s.firmware_job.job.running());
    }

    #[test]
    fn opening_the_firmware_screen_probes_unless_a_job_is_recent() {
        use carpi_proto::FirmwareAction as A;
        let (client, rx) = asking_client();
        client.snapshot().connected = true;
        client.firmware_opened();
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Firmware(A::Status)))
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Firmware(A::Probe)))
        ));
        // Running: status only.
        client.firmware_opened();
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Firmware(A::Status)))
        ));
        assert!(rx.try_recv().is_err());
        // Ended just now: status only.
        client.snapshot().firmware_event(JobEvent::Finished {
            ok: true,
            message: String::new(),
        });
        client.firmware_opened();
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Firmware(A::Status)))
        ));
        assert!(rx.try_recv().is_err());
        // A minute later: a fresh probe.
        client.snapshot().firmware_job.last =
            Instant::now().checked_sub(FirmwareJob::QUIET + Duration::from_secs(1));
        client.firmware_opened();
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Firmware(A::Status)))
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Firmware(A::Probe)))
        ));
    }

    #[test]
    fn fixture_refuses_firmware_jobs() {
        let fixture: Fixture =
            serde_json::from_str(include_str!("../fixtures/firmware-newer.json")).unwrap();
        let client = fixture.start();
        client.firmware(carpi_proto::FirmwareAction::Probe);
        let until = Instant::now() + Duration::from_secs(5);
        while client.snapshot().firmware_job.job.running() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        let s = client.snapshot();
        assert_eq!(
            s.firmware_job.job.finished,
            Some((false, "firmware jobs need a live carpid connection".into()))
        );
        assert_eq!(
            s.firmware.rnse.comparison,
            crate::firmware::Comparison::Newer
        );
    }

    fn installed(running: Option<bool>, outcome: Option<&str>) -> carpi_proto::Installed {
        carpi_proto::Installed {
            current: Some("8f2a408".into()),
            previous: Some("6e988a2".into()),
            downloaded: Some("6e988a2".into()),
            last_apply: outcome.map(str::to_string),
            update_running: running,
            ..Default::default()
        }
    }

    #[test]
    fn restart_recovers_persisted_success_without_reusing_the_old_job() {
        let mut s = Snapshot::default();
        s.start_update(UpdateAction::Apply, false);
        s.update = UpdateJob {
            job: Some(7),
            stage: "restart".into(),
            message: "restarting services".into(),
            ..Default::default()
        };
        s.update_disconnected();
        assert!(s.update.running());
        assert_eq!(s.update.job, None);
        assert_ne!(s.update.message, "restarting services");
        s.recover_update(1, installed(Some(false), Some("ok")));
        assert!(!s.update.running());
        assert_eq!(s.update.finished, Some((true, "8f2a408 installed".into())));
        assert_eq!(s.update_status.unwrap().current.as_deref(), Some("8f2a408"));
    }

    #[test]
    fn local_attempt_observes_apply_before_recovering_its_result() {
        for (outcome, ok, message) in [
            ("ok", true, "8f2a408 installed"),
            ("rolled_back", false, "Update rolled back; running 8f2a408"),
            ("failed", false, "Update failed; running 8f2a408"),
            ("applying", false, "Update was interrupted; running 8f2a408"),
        ] {
            let mut s = Snapshot::default();
            s.start_update(UpdateAction::Fetch, true);
            s.update_disconnected();
            s.recover_update(1, installed(Some(true), Some("applying")));
            assert!(s.update.running());
            s.recover_update(1, installed(Some(false), Some(outcome)));
            assert!(!s.update.running());
            assert_eq!(s.update.finished, Some((ok, message.into())));
        }
    }

    #[test]
    fn fresh_client_treats_idle_last_apply_as_history() {
        for outcome in ["ok", "rolled_back", "failed", "applying"] {
            let mut s = Snapshot::default();
            s.recover_update(0, installed(Some(false), Some(outcome)));
            assert!(!s.update.running());
            assert!(s.update.finished.is_none());
            assert_eq!(
                s.update_status.unwrap().last_apply.as_deref(),
                Some(outcome)
            );
        }
    }

    #[test]
    fn fresh_client_busy_then_fetch_failure_cannot_reuse_previous_apply_success() {
        let mut s = Snapshot::default();
        s.update_status_failed(0, "an update or Wi-Fi job is running".into());
        assert!(!s.update.running());
        assert!(s.update_attempt.is_none());
        assert!(s.update_recovering);
        assert!(s.update.finished.is_none());
        // Fetch failed without reaching switch(), so the persisted apply result is unchanged.
        s.recover_update(0, installed(Some(false), Some("ok")));
        assert!(!s.update.running());
        // This UI never started the failed fetch: only the previous apply is known.
        assert!(s.update.finished.is_none());
        assert!(s.update_note.is_none());
        assert_eq!(s.update_status.unwrap().last_apply.as_deref(), Some("ok"));
    }

    #[test]
    fn active_apply_never_uses_previous_success_and_keeps_waiting_for_real_result() {
        let mut s = Snapshot::default();
        s.start_update(UpdateAction::Apply, false);
        s.update_disconnected();
        s.recover_update(1, installed(Some(true), Some("ok")));
        assert!(s.update.running());
        assert!(s.update.finished.is_none());
        assert!(s.update_recovering);
        s.recover_update(1, installed(Some(true), Some("applying")));
        assert!(s.update.running());
        s.recover_update(1, installed(Some(false), Some("rolled_back")));
        assert!(!s.update.running());
        assert!(!s.update.finished.as_ref().unwrap().0);
    }

    #[test]
    fn legacy_status_cannot_prove_an_interrupted_apply_succeeded() {
        let mut s = Snapshot::default();
        s.start_update(UpdateAction::Apply, false);
        s.update_disconnected();
        s.recover_update(1, installed(None, Some("ok")));
        assert!(!s.update.running());
        assert!(s.update.finished.is_none());
        assert!(s.update_note.unwrap().contains("outcome unknown"));
    }

    #[test]
    fn status_from_before_a_new_update_cannot_finish_it() {
        let (tx, rx) = mpsc::channel();
        let client = Client {
            shared: Arc::new(Mutex::new(Snapshot::default())),
            asks: Some(tx),
            watch: Arc::default(),
        };
        {
            let mut s = client.snapshot();
            s.update.finished = Some((false, "old failure".into()));
            s.update_note = Some("old unknown result".into());
            s.update_apply_observed = true;
        }
        client.update(UpdateAction::Fetch, true);
        assert!(matches!(
            rx.try_recv(),
            Ok(Ev::Ask(Ask::Update(UpdateAction::Fetch, true)))
        ));
        let mut s = client.snapshot();
        assert!(s.update.finished.is_none());
        assert!(s.update_note.is_none());
        assert!(!s.update_apply_observed);
        let before = s.update.clone();
        s.recover_update(0, installed(Some(false), Some("ok")));
        s.recover_update(1, installed(Some(false), Some("rolled_back")));
        s.update_status_failed(0, "old status error".into());
        assert_eq!(s.update, before);
        assert!(s.update_status.is_none());
        s.update_disconnected();
        let before = s.update.clone();
        s.recover_update(0, installed(Some(false), Some("ok")));
        s.update_status_failed(0, "old status error during recovery".into());
        assert_eq!(s.update, before);
        assert!(s.update_status.is_none());
    }

    #[test]
    fn disconnect_before_apply_does_not_report_the_previous_apply_as_success() {
        let mut s = Snapshot::default();
        s.update_status = Some(installed(Some(false), Some("ok")));
        s.start_update(UpdateAction::Fetch, true);
        s.update = UpdateJob {
            job: Some(7),
            stage: "download".into(),
            ..Default::default()
        };
        s.update_disconnected();
        s.update_status_failed(1, "an update or Wi-Fi job is running".into());
        assert!(s.update.running());
        s.recover_update(1, installed(Some(true), Some("ok")));
        assert!(s.update.running());
        s.recover_update(1, installed(Some(false), Some("ok")));
        assert!(!s.update.running());
        assert!(s.update.finished.is_none());
        assert!(s.update_note.as_ref().unwrap().contains("outcome unknown"));
        assert_eq!(
            s.update_status.as_ref().unwrap().last_apply.as_deref(),
            Some("ok")
        );
        // A manual refresh must not turn unchanged history into a successful local attempt.
        s.update_recovering = true;
        s.recover_update(1, installed(Some(false), Some("ok")));
        assert!(s.update.finished.is_none());
        assert!(s.update_note.is_some());
    }

    #[test]
    fn passive_busy_and_live_status_during_old_updater_handoff_accept_installed_history() {
        for (busy_first, live_reply) in [(true, false), (false, true), (true, true)] {
            let mut s = Snapshot::default();
            if busy_first {
                s.update_status_failed(0, "an update or Wi-Fi job is running".into());
            }
            if live_reply {
                // The older updater retains its previous terminal result while holding the lock.
                s.recover_update(0, installed(Some(true), Some("ok")));
                assert!(s.update_recovering);
                assert!(!s.update.running());
                s.update_disconnected();
                s.update_status_failed(0, "busy during handoff".into());
            }
            let status = carpi_proto::Installed {
                current: Some("beta-20261109-5e0cf2c".into()),
                previous: Some("8f2a408".into()),
                downloaded: None,
                last_apply: Some("ok".into()),
                update_running: Some(false),
                ..Default::default()
            };
            s.recover_update(0, status.clone());
            assert_eq!(s.update_status, Some(status));
            assert_eq!(s.update, UpdateJob::default());
            assert!(s.update_attempt.is_none());
            assert!(s.update_note.is_none());
            assert!(!s.update_recovering);
        }
    }

    #[test]
    fn passive_apply_marker_remains_history_without_inventing_a_local_job() {
        let mut s = Snapshot::default();
        s.recover_update(0, installed(Some(true), Some("applying")));
        s.recover_update(0, installed(Some(false), Some("ok")));
        assert_eq!(s.update, UpdateJob::default());
        assert!(s.update_note.is_none());
    }

    #[test]
    fn passive_live_fetch_followed_by_previous_success_remains_history() {
        let mut s = Snapshot::default();
        s.recover_update(0, installed(Some(true), Some("ok")));
        assert!(!s.update.running());
        s.recover_update(0, installed(Some(false), Some("ok")));
        assert_eq!(s.update, UpdateJob::default());
        assert!(s.update_note.is_none());
    }

    #[test]
    fn local_apply_missing_stages_confirms_a_correlated_new_release() {
        for action in [UpdateAction::Apply, UpdateAction::Fetch] {
            let mut s = Snapshot::default();
            let mut baseline = installed(Some(false), Some("ok"));
            baseline.downloaded = Some("beta-20261109-5e0cf2c".into());
            s.update_status = Some(baseline);
            s.start_update(action, true);
            s.update_disconnected();
            s.recover_update(1, installed(Some(true), Some("ok")));
            assert!(s.update.finished.is_none());
            s.recover_update(
                1,
                carpi_proto::Installed {
                    current: Some("beta-20261109-5e0cf2c".into()),
                    // Apply knows the downloaded target; fetch can use the previous release.
                    previous: (action == UpdateAction::Fetch).then(|| "8f2a408".into()),
                    downloaded: None,
                    last_apply: Some("ok".into()),
                    update_running: Some(false),
                    ..Default::default()
                },
            );
            assert_eq!(
                s.update.finished,
                Some((true, "beta-20261109-5e0cf2c installed".into()))
            );
            assert!(s.update_note.is_none());
        }
    }

    #[test]
    fn unchanged_target_or_unrelated_release_cannot_reuse_previous_success() {
        for current in ["8f2a408", "unrelated-release"] {
            let mut s = Snapshot::default();
            s.update_status = Some(installed(Some(false), Some("ok")));
            s.start_update(UpdateAction::Apply, false);
            s.update_disconnected();
            let mut status = installed(Some(false), Some("ok"));
            status.current = Some(current.into());
            s.recover_update(1, status);
            assert!(s.update.finished.is_none());
            assert!(s.update_note.unwrap().contains("outcome unknown"));
        }
    }

    #[test]
    fn new_failure_history_confirms_a_local_apply_that_missed_stages() {
        for outcome in ["failed", "rolled_back"] {
            let mut s = Snapshot::default();
            s.update_status = Some(installed(Some(false), Some("ok")));
            s.start_update(UpdateAction::Fetch, true);
            s.update_disconnected();
            s.recover_update(1, installed(Some(false), Some(outcome)));
            assert!(!s.update.finished.unwrap().0);
            assert!(s.update_note.is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn reconnect_polls_until_detached_apply_finishes_and_keeps_status_jobs_separate() {
        use carpi_proto::{Request, Response};
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixListener, UnixStream};

        fn request(s: &mut UnixStream) -> Request {
            let mut prefix = [0; 4];
            s.read_exact(&mut prefix).unwrap();
            let mut body = vec![0; carpi_proto::body_len(prefix).unwrap()];
            s.read_exact(&mut body).unwrap();
            carpi_proto::decode(&body).unwrap()
        }
        fn reply(s: &mut UnixStream, response: Response) {
            s.write_all(&carpi_proto::encode(&response)).unwrap();
        }
        fn hello(s: &mut UnixStream) {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            assert!(matches!(request(s), Request::Hello { .. }));
            reply(
                s,
                Response::Hello {
                    version: carpi_proto::PROTOCOL_VERSION,
                    daemon: "restarted carpid".into(),
                },
            );
            assert_eq!(request(s), Request::Catalog);
            reply(
                s,
                Response::Catalog {
                    categories: vec![],
                    values: vec![],
                },
            );
        }
        fn accept(listener: &UnixListener) -> UnixStream {
            listener.set_nonblocking(true).unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => return stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("carpid reconnect: {e}"),
                }
            }
        }
        fn next_update(s: &mut UnixStream) -> Request {
            loop {
                match request(s) {
                    Request::Values { .. } => reply(s, Response::Values { items: vec![] }),
                    request @ Request::Update { .. } => return request,
                    other => panic!("unexpected request {other:?}"),
                }
            }
        }
        fn status(s: &mut UnixStream, job: u32, installed: carpi_proto::Installed) {
            assert_eq!(
                next_update(s),
                Request::Update {
                    action: UpdateAction::Status,
                    and_apply: false,
                }
            );
            reply(s, Response::JobStarted { job });
            reply(
                s,
                Response::Job {
                    job,
                    event: JobEvent::Installed(installed),
                },
            );
            // This success means status was read, even when the apply rolled back.
            reply(
                s,
                Response::Job {
                    job,
                    event: JobEvent::Finished {
                        ok: true,
                        message: "status".into(),
                    },
                },
            );
        }

        for (outcome, ok) in [("ok", true), ("rolled_back", false)] {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "carpi-reconnect-{}-{unique}.sock",
                std::process::id()
            ));
            let listener = UnixListener::bind(&path).unwrap();
            let client = Client::connect(path.clone());
            let mut stream = accept(&listener);
            hello(&mut stream);
            status(
                &mut stream,
                1,
                carpi_proto::Installed {
                    update_running: Some(false),
                    ..Default::default()
                },
            );
            assert!(wait(&client, |s| s.update_status.is_some()));
            client.update(UpdateAction::Apply, false);
            assert_eq!(
                next_update(&mut stream),
                Request::Update {
                    action: UpdateAction::Apply,
                    and_apply: false,
                }
            );
            reply(&mut stream, Response::JobStarted { job: 2 });
            reply(
                &mut stream,
                Response::Job {
                    job: 2,
                    event: JobEvent::Stage {
                        stage: "restart".into(),
                        message: "restarting services".into(),
                    },
                },
            );
            assert!(wait(&client, |s| s.update.stage == "restart"));
            drop(stream);
            assert!(wait(&client, |s| !s.connected));
            assert!(client.snapshot().update.finished.is_none());

            let mut stream = accept(&listener);
            hello(&mut stream);
            // Reuse the interrupted job number, as a newly started daemon can.
            status(&mut stream, 2, installed(Some(true), Some("ok")));
            assert!(wait(&client, |s| s.update.message
                == "Waiting for the updater to finish"));
            assert!(client.snapshot().update.finished.is_none());
            status(&mut stream, 3, installed(Some(false), Some(outcome)));
            assert!(wait(&client, |s| s
                .update
                .finished
                .as_ref()
                .is_some_and(|f| f.0 == ok)));
            assert!(!client.snapshot().update.running());
            assert_ne!(
                client.snapshot().update.finished.as_ref().unwrap().1,
                "status"
            );
            drop(listener);
            drop(stream);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn flashing_mode_announces_startup_and_changes_without_repeating() {
        let mut snap = Snapshot::default();
        let mut settings = crate::settings::sample();
        snap.take_settings(settings.clone());
        assert!(snap.toast.is_none());
        settings.flashing_mode = true;
        snap.take_settings(settings.clone());
        let toast = snap.toast.clone().unwrap();
        assert_eq!(toast.level, ToastLevel::Warn);
        assert!(toast.text.contains("CAN sends blocked"));
        snap.take_settings(settings.clone());
        assert_eq!(snap.toast, Some(toast));

        let mut startup = Snapshot::default();
        startup.take_settings(settings.clone());
        assert!(startup.toast.unwrap().text.contains("Flashing mode ON"));
        settings.flashing_mode = false;
        snap.take_settings(settings);
        assert_eq!(snap.toast.unwrap().text, "Flashing mode OFF");
    }

    #[test]
    fn lightweight_flashing_status_updates_cached_settings_and_keeps_warning_when_unknown() {
        let mut snap = Snapshot::default();
        snap.take_settings(crate::settings::sample());
        snap.take_values(
            vec![Reading {
                name: "system.flashing_mode".into(),
                value: Some(carpi_proto::Value::Bool(true)),
                age_ms: 0,
            }],
            Instant::now(),
        );
        assert!(snap.flashing_mode && snap.settings.as_ref().unwrap().flashing_mode);
        let toast = snap.toast.clone();
        snap.take_values(
            vec![Reading {
                name: "system.flashing_mode".into(),
                value: None,
                age_ms: 4000,
            }],
            Instant::now(),
        );
        assert!(snap.flashing_mode);
        assert_eq!(snap.toast, toast);
    }

    fn wait(client: &Client, what: impl Fn(&Snapshot) -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if what(&client.snapshot()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn the_fixture_reads() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/car.json");
        let client = Fixture::read(&path).unwrap().start();
        let s = client.snapshot();
        assert!(s.connected);
        assert!(s.values.get("engine.rpm").is_some());
        assert_eq!(s.values.get("wheels.rl_pulses").unwrap().value, None);
        assert_eq!(s.buses.as_ref().unwrap()[0].ring.len(), 4);
        assert_eq!(s.settings.as_ref().unwrap().bindings.len(), 6);
        let Some(carpi_proto::Value::Text(curve)) = &s.values.get("light.curve").unwrap().value
        else {
            panic!("the fixture has a curve")
        };
        assert_eq!(crate::light::parse(curve).unwrap().points.len(), 7);
        drop(s);
        client.capture();
        assert!(wait(&client, |s| matches!(
            s.capture,
            Capture::Saved { .. }
        )));
        let toast = client.snapshot().toast.clone().unwrap();
        assert_eq!(toast.text, "Capture 2026-11-08 #01");
    }

    #[cfg(unix)]
    #[test]
    fn says_hello_subscribes_while_shown_and_saves_captures() {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixListener, UnixStream};

        use carpi_proto::{Request, Response, Value};

        fn request(s: &mut UnixStream) -> Request {
            let mut prefix = [0u8; 4];
            s.read_exact(&mut prefix).unwrap();
            let mut body = vec![0u8; carpi_proto::body_len(prefix).unwrap()];
            s.read_exact(&mut body).unwrap();
            carpi_proto::decode(&body).unwrap()
        }
        fn reply(s: &mut UnixStream, r: &Response) {
            s.write_all(&carpi_proto::encode(r)).unwrap();
        }

        let dir = std::env::temp_dir().join(format!("carpi-ui-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("carpid.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let client = Client::connect(path);
        let (mut s, _) = listener.accept().unwrap();
        assert!(matches!(request(&mut s), Request::Hello { .. }));
        reply(
            &mut s,
            &Response::Hello {
                version: carpi_proto::PROTOCOL_VERSION,
                daemon: "carpid test".into(),
            },
        );
        assert!(wait(&client, |s| s.connected));
        assert_eq!(request(&mut s), Request::Catalog);
        reply(
            &mut s,
            &Response::Catalog {
                categories: vec!["Engine".into()],
                values: vec![],
            },
        );
        assert_eq!(
            request(&mut s),
            Request::Update {
                action: UpdateAction::Status,
                and_apply: false
            }
        );
        reply(&mut s, &Response::JobStarted { job: 1 });
        reply(
            &mut s,
            &Response::Job {
                job: 1,
                event: JobEvent::Installed(carpi_proto::Installed {
                    update_running: Some(false),
                    ..Default::default()
                }),
            },
        );
        reply(
            &mut s,
            &Response::Job {
                job: 1,
                event: JobEvent::Finished {
                    ok: true,
                    message: "status".into(),
                },
            },
        );

        // With every page hidden, persistent flashing mode must still reach the shell.
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(
            request(&mut s),
            Request::Values {
                prefix: "system.flashing_mode".into()
            }
        );
        reply(
            &mut s,
            &Response::Values {
                items: vec![Reading {
                    name: "system.flashing_mode".into(),
                    value: Some(Value::Bool(true)),
                    age_ms: 0,
                }],
            },
        );
        assert!(wait(&client, |s| s
            .toast
            .as_ref()
            .is_some_and(|t| t.text.contains("Flashing mode ON"))));

        client.shown(true);
        assert!(matches!(request(&mut s), Request::Subscribe { sub: 1, .. }));
        reply(&mut s, &Response::Ok);
        assert_eq!(
            request(&mut s),
            Request::Settings {
                action: carpi_proto::SettingsAction::Get
            }
        );
        reply(
            &mut s,
            &Response::Settings {
                settings: crate::settings::sample(),
                restart_needed: vec!["tx".into()],
            },
        );
        assert_eq!(request(&mut s), Request::Values { prefix: "".into() });
        reply(&mut s, &Response::Values { items: vec![] });
        assert_eq!(request(&mut s), Request::Bus);
        reply(&mut s, &Response::Bus { buses: vec![] });
        assert_eq!(
            request(&mut s),
            Request::Datalog {
                action: DatalogAction::Status
            }
        );
        reply(
            &mut s,
            &Response::Datalog {
                status: Box::new(crate::logging::tests::status()),
            },
        );
        reply(
            &mut s,
            &Response::Update {
                sub: 1,
                items: vec![Reading {
                    name: "engine.rpm".into(),
                    value: Some(Value::Num(800.0)),
                    age_ms: 5,
                }],
            },
        );
        assert!(wait(&client, |s| s.values.get("engine.rpm").is_some()));
        assert!(wait(&client, |s| s
            .datalog
            .as_ref()
            .is_some_and(|d| d.profiles.len() == 2)));
        assert!(wait(&client, |s| s.settings.is_some()
            && s.restart_needed == ["tx"]));

        client.set(carpi_proto::SettingsPatch {
            dis_enabled: Some(false),
            ..Default::default()
        });
        loop {
            match request(&mut s) {
                Request::Settings {
                    action: carpi_proto::SettingsAction::Set { patch },
                } => {
                    assert_eq!(patch.dis_enabled, Some(false));
                    break;
                }
                Request::Values { .. } => reply(&mut s, &Response::Values { items: vec![] }),
                Request::Bus => reply(&mut s, &Response::Bus { buses: vec![] }),
                Request::Datalog { .. } => reply(
                    &mut s,
                    &Response::Datalog {
                        status: Box::default(),
                    },
                ),
                other => panic!("{other:?}"),
            }
        }
        reply(
            &mut s,
            &Response::Error {
                message: "dis: bad".into(),
            },
        );
        assert!(wait(&client, |s| s.settings_note.as_deref() == Some("dis: bad")));

        // The diagnostics page: its catalog names, and a job with its events.
        client.watch(vec!["engine.maf_gs".into()]);
        client.watch(vec!["engine.maf_gs".into()]);
        client.diag(carpi_proto::DiagAction::InstallList);
        let mut watched = false;
        loop {
            match request(&mut s) {
                Request::Subscribe {
                    sub: 2, prefixes, ..
                } => {
                    assert!(!watched, "asked once for the same names");
                    assert_eq!(prefixes, ["engine.maf_gs"]);
                    watched = true;
                    reply(&mut s, &Response::Ok);
                }
                Request::Diag {
                    action: carpi_proto::DiagAction::InstallList,
                } => break,
                Request::Values { .. } => reply(&mut s, &Response::Values { items: vec![] }),
                Request::Bus => reply(&mut s, &Response::Bus { buses: vec![] }),
                Request::Datalog { .. } => reply(
                    &mut s,
                    &Response::Datalog {
                        status: Box::default(),
                    },
                ),
                other => panic!("{other:?}"),
            }
        }
        assert!(watched);
        reply(&mut s, &Response::JobStarted { job: 7 });
        reply(
            &mut s,
            &Response::Job {
                job: 7,
                event: carpi_proto::JobEvent::InstallList {
                    modules: vec![carpi_proto::InstalledModule {
                        vcds: 0x01,
                        tp2: 0x01,
                        coded: true,
                        status: 0,
                    }],
                },
            },
        );
        reply(
            &mut s,
            &Response::Job {
                job: 7,
                event: carpi_proto::JobEvent::Finished {
                    ok: true,
                    message: String::new(),
                },
            },
        );
        assert!(wait(&client, |s| !s.diag.busy()
            && s.diag.installed.as_ref().is_some_and(|l| l.len() == 1)));

        client.datalog(DatalogAction::Start {
            profile: "Boost".into(),
        });
        loop {
            match request(&mut s) {
                Request::Datalog {
                    action: DatalogAction::Start { profile },
                } => {
                    assert_eq!(profile, "Boost");
                    break;
                }
                Request::Values { .. } => reply(&mut s, &Response::Values { items: vec![] }),
                Request::Bus => reply(&mut s, &Response::Bus { buses: vec![] }),
                Request::Datalog { .. } => reply(
                    &mut s,
                    &Response::Datalog {
                        status: Box::default(),
                    },
                ),
                other => panic!("{other:?}"),
            }
        }
        reply(
            &mut s,
            &Response::Error {
                message: "already logging 2026-11-08 #03".into(),
            },
        );
        assert!(wait(&client, |s| s.datalog_note.as_deref()
            == Some("already logging 2026-11-08 #03")));

        client.capture();
        // Polls may come first.
        loop {
            match request(&mut s) {
                Request::Capture { trigger } => {
                    assert_eq!(trigger, "page");
                    break;
                }
                Request::Values { .. } => reply(&mut s, &Response::Values { items: vec![] }),
                Request::Bus => reply(&mut s, &Response::Bus { buses: vec![] }),
                Request::Datalog { .. } => reply(
                    &mut s,
                    &Response::Datalog {
                        status: Box::default(),
                    },
                ),
                other => panic!("{other:?}"),
            }
        }
        reply(
            &mut s,
            &Response::Captured {
                date: "2026-11-08".into(),
                number: 3,
                label: "2026-11-08 #03".into(),
                dir: "/home/pi/logs/2026-11-08/03".into(),
                summary: "CAN, journal, values".into(),
            },
        );
        assert!(wait(&client, |s| s
            .toast
            .as_ref()
            .is_some_and(|t| t.text == "Capture 2026-11-08 #03")));
        drop(s);
        assert!(wait(&client, |s| !s.connected));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
