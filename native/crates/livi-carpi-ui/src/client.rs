// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pages' own connection to carpid, on a thread of its own.
//!
//! Same socket and protocol as livi-carpi (`$CARPID_SOCKET` or `$XDG_RUNTIME_DIR/carpid.sock`,
//! length-prefixed JSON). It stays connected, but only asks for anything while one of our pages is
//! on screen: a subscription to every value, plus `Values` and `Bus` once a second so ages stay
//! true, and carpid's settings each time a page comes up. `CARPI_UI_FIXTURE=<file.json>` shows a
//! fixed set instead, for snapshots without carpid.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use carpi_proto::{BusInfo, LightAction, Reading, Settings, SettingsPatch};
use serde::Deserialize;

use crate::diag::Diag;
use crate::values::Store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    Idle,
    Saving,
    Saved { path: String, frames: usize },
    Failed(String),
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
    /// Empty until carpid has a diag API.
    pub diag: Diag,
    /// carpid's settings, from the last `Settings` reply.
    pub settings: Option<Settings>,
    /// Settings saved that only take effect after a restart of carpid.
    pub restart_needed: Vec<String>,
    /// Why the last settings change didn't go through, or a note about one.
    pub settings_note: Option<String>,
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
        }
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
enum Ask {
    Shown(bool),
    SaveCapture,
    /// `None` asks for the settings, a patch changes them.
    Settings(Option<Box<SettingsPatch>>),
    Light(LightAction),
}

#[derive(Clone)]
pub struct Client {
    shared: Arc<Mutex<Snapshot>>,
    asks: Option<Sender<Ev>>,
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
        let (tx, rx) = mpsc::channel();
        let inner = shared.clone();
        let ev = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("carpi-ui".into())
            .spawn(move || link::run(&socket, inner, ev, rx));
        match spawned {
            Ok(_) => Self {
                shared,
                asks: Some(tx),
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

    pub fn save_capture(&self) {
        if self.asks.is_none() {
            self.snapshot().capture = Capture::Failed("not connected".into());
        }
        self.ask(Ask::SaveCapture);
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
        Ok,
    }

    pub(super) fn run(
        socket: &Path,
        shared: Arc<Mutex<Snapshot>>,
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
                    session(id, stream, &shared, &tx, &rx, &mut shown)
                }
                Err(e) => format!("carpid not running ({e})"),
            };
            {
                let mut s = lock(&shared);
                s.connected = false;
                s.problem = Some(why);
                if s.capture == Capture::Saving {
                    s.capture = Capture::Failed("carpid went away".into());
                }
            }
            let until = Instant::now() + retry;
            retry = (retry * 2).min(RETRY_MAX);
            // Keep taking asks while waiting.
            while let Some(left) = until.checked_duration_since(Instant::now()) {
                match rx.recv_timeout(left) {
                    Ok(Ev::Ask(Ask::Shown(on))) => shown = on,
                    Ok(Ev::Ask(Ask::SaveCapture)) => {
                        lock(&shared).capture = Capture::Failed("not connected".into())
                    }
                    Ok(Ev::Ask(Ask::Settings(Some(_)) | Ask::Light(_))) => {
                        lock(&shared).settings_note = Some("not connected".into())
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
        };
        let why = link.drive(id, shared, rx, shown);
        let _ = link.stream.shutdown(std::net::Shutdown::Both);
        why
    }

    struct Link {
        stream: UnixStream,
        pending: VecDeque<Pending>,
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
            self.send(&Request::Bus, Pending::Bus)
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
                self.poll()
            } else {
                self.send(&Request::Unsubscribe { sub: SUB }, Pending::Ok)
            }
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
                    Ok(Ev::Ask(Ask::SaveCapture)) => {
                        lock(shared).capture = Capture::Saving;
                        self.send(&Request::SaveCapture { path: None }, Pending::Capture)
                    }
                    Ok(Ev::Ask(Ask::Settings(patch))) => self.settings(patch),
                    Ok(Ev::Ask(Ask::Light(action))) => {
                        self.send(&Request::Light { action }, Pending::Light)
                    }
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
                // Not ours: only the bridge asks for commands or starts jobs.
                Response::Command { .. } | Response::Job { .. } => return Ok(()),
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
                }
                (Some(Pending::Hello), Response::Error { message }) => {
                    return Err(format!("carpid refused: {message}"));
                }
                (_, Response::Values { items }) => s.values.take(items, now),
                (_, Response::Bus { buses }) => s.buses = Some(buses),
                (_, Response::CaptureSaved { path, frames }) => {
                    s.capture = Capture::Saved { path, frames }
                }
                (Some(Pending::Capture), Response::Error { message }) => {
                    s.capture = Capture::Failed(message)
                }
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
        }
        let (tx, rx) = mpsc::channel();
        let inner = shared.clone();
        let _ = std::thread::Builder::new()
            .name("carpi-ui-fixture".into())
            .spawn(move || self.run(&inner, &rx));
        Client {
            shared,
            asks: Some(tx),
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
                Ok(Ev::Ask(Ask::SaveCapture)) => {
                    s.capture = Capture::Saved {
                        path: "fixture.log".into(),
                        frames: 0,
                    }
                }
                Ok(Ev::Ask(Ask::Settings(Some(patch)))) => {
                    if let Some(settings) = &mut s.settings {
                        crate::settings::apply(settings, &patch);
                    }
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
        client.save_capture();
        assert!(wait(&client, |s| matches!(
            s.capture,
            Capture::Saved { .. }
        )));
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

        client.save_capture();
        // Polls may come first.
        loop {
            match request(&mut s) {
                Request::SaveCapture { path: None } => break,
                Request::Values { .. } => reply(&mut s, &Response::Values { items: vec![] }),
                Request::Bus => reply(&mut s, &Response::Bus { buses: vec![] }),
                other => panic!("{other:?}"),
            }
        }
        reply(
            &mut s,
            &Response::CaptureSaved {
                path: "/x/cap.log".into(),
                frames: 9,
            },
        );
        assert!(wait(&client, |s| s.capture
            == Capture::Saved {
                path: "/x/cap.log".into(),
                frames: 9
            }));
        drop(s);
        assert!(wait(&client, |s| !s.connected));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
