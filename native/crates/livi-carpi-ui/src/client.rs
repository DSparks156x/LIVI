// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pages' own connection to carpid, on a thread of its own.
//!
//! Same socket and protocol as livi-carpi (`$CARPID_SOCKET` or `$XDG_RUNTIME_DIR/carpid.sock`,
//! length-prefixed JSON). It stays connected, but only asks for anything while one of our pages is
//! on screen: a subscription to every value, plus `Values` and `Bus` once a second so ages stay
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
    /// Saved Wi-Fi networks and the last scan or test.
    pub wifi: Wifi,
    /// Where the system runs from, and Move to NVMe.
    pub storage: Storage,
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
            wifi: Wifi::default(),
            storage: Storage::default(),
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
    Wifi(carpi_proto::WifiAction),
    Storage(carpi_proto::StorageAction),
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

    /// Starts an update job; its events land in [`Snapshot::update`].
    pub fn update(&self, action: UpdateAction, and_apply: bool) {
        {
            let mut s = self.snapshot();
            s.update = UpdateJob {
                asked: true,
                ..Default::default()
            };
            if self.asks.is_none() {
                s.update.refused("not connected".into());
            }
        }
        self.ask(Ask::Update(action, and_apply));
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
        /// List, add, forget.
        Wifi,
        /// Scan, test.
        WifiJob,
        Storage,
        /// Move to NVMe.
        StorageJob,
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
    }

    impl Link {
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
                        if *shown { self.poll() } else { Ok(()) }
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
                    s.values.take(items, now);
                    return Ok(());
                }
                Response::Job { job, event } => {
                    if s.wifi.job.job == Some(job) {
                        if let JobEvent::WifiSeen { networks } = &event {
                            s.wifi.seen = Some(networks.clone());
                        }
                        s.wifi.job.event(event);
                    } else if s.update.job == Some(job) {
                        s.update.event(event);
                    } else if s.storage.job.job == Some(job) {
                        s.storage.job.event(event);
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
                (_, Response::Values { items }) => s.values.take(items, now),
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
                    s.settings = Some(settings);
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
            s.settings = self.settings.clone();
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
        s.values.take(self.values.clone(), Instant::now());
        s.buses = self.buses.clone();
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
                    if let Some(settings) = &mut s.settings {
                        crate::settings::apply(settings, &patch);
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
