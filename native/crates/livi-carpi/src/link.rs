// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection to carpid, kept up for as long as the core runs.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use carpi_proto::{ApAction, Command, InputAction, PROTOCOL_VERSION, Request, Response};
use livi_core_proto::state::{Front, State};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{broadcast, mpsc, watch};

use crate::ap::{Ap, Service, Systemd};
use crate::bridge::Sent;
use crate::input;
use crate::values::{self, Values};

const CLIENT: &str = "livi";
const SUB: u32 = 1;
/// carpid sends a changed value at most this often.
const PERIOD_MS: u32 = 200;
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);

fn socket() -> PathBuf {
    if let Some(path) = std::env::var_os("CARPID_SOCKET") {
        return path.into();
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(dir).join("carpid.sock")
}

/// What the bridge takes from the core.
pub struct Hooks {
    pub state: watch::Receiver<State>,
    /// The core's own telemetry input.
    pub telemetry: mpsc::UnboundedSender<Value>,
    /// The core's switch that holds paging while the hotspot is down.
    pub ap_ready: watch::Sender<bool>,
    /// The UI path livi-ui reports, which says when one of our pages is shown.
    pub ui_path: watch::Receiver<String>,
    /// Shows a UI path, like the core's `open_page`.
    pub open_page: broadcast::Sender<String>,
    /// Hands an input action to the projection, which maps it for the active session.
    pub input: Box<dyn Fn(InputAction) + Send + Sync>,
}

/// The UI path, and where carpid's input actions go.
struct Outlets {
    ui_path: watch::Receiver<String>,
    open_page: broadcast::Sender<String>,
    input: Box<dyn Fn(InputAction) + Send + Sync>,
}

/// Runs until the core's state goes away.
pub async fn run(hooks: Hooks) {
    let Hooks {
        mut state,
        telemetry,
        ap_ready,
        ui_path,
        open_page,
        input,
    } = hooks;
    let out = Outlets {
        ui_path,
        open_page,
        input,
    };
    let path = socket();
    let tank_l = values::tank_l();
    let mut ap = Ap::new(
        Systemd {
            state: state.clone(),
        },
        Some(ap_ready),
    );
    let mut retry = RETRY_FIRST;
    let mut last_error = String::new();
    loop {
        let mut values = Values::default();
        let mut connected = false;
        let result = session(
            &path,
            &mut state,
            &telemetry,
            &mut values,
            tank_l,
            &mut connected,
            &mut ap,
            &out,
        )
        .await;
        if let Some(patch) = values.forget() {
            let _ = telemetry.send(patch);
        }
        // carpid can't tell us to resume any more.
        if let Err(e) = ap.resume().await {
            eprintln!("[carpi] hotspot: {e}");
        }
        if connected {
            retry = RETRY_FIRST;
            last_error.clear();
        }
        match result {
            Ok(Ended::StateGone) => return,
            Ok(Ended::Closed) => println!("[carpi] carpid closed the connection"),
            Err(e) => {
                let e = e.to_string();
                if e != last_error {
                    eprintln!("[carpi] {}: {e}", path.display());
                    last_error = e;
                }
            }
        }
        let wait = tokio::time::sleep(retry);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                changed = state.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
        retry = (retry * 2).min(RETRY_MAX);
    }
}

enum Ended {
    StateGone,
    Closed,
}

#[allow(clippy::too_many_arguments)]
async fn session(
    path: &Path,
    state: &mut watch::Receiver<State>,
    telemetry: &mpsc::UnboundedSender<Value>,
    values: &mut Values,
    tank_l: f64,
    connected: &mut bool,
    ap: &mut Ap<impl Service>,
    out: &Outlets,
) -> io::Result<Ended> {
    let stream = UnixStream::connect(path).await?;
    let (mut reader, mut writer) = stream.into_split();
    send(
        &mut writer,
        &Request::Hello {
            version: PROTOCOL_VERSION,
            client: CLIENT.into(),
        },
    )
    .await?;
    match tokio::time::timeout(HELLO_TIMEOUT, read(&mut reader)).await {
        Ok(Ok(Response::Hello { daemon, .. })) => {
            println!("[carpi] connected to {daemon}");
            *connected = true;
        }
        Ok(Ok(Response::Error { message })) => return Err(io::Error::other(message)),
        Ok(Ok(other)) => return Err(io::Error::other(format!("no hello: {other:?}"))),
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(io::Error::other("no hello")),
    }
    send(
        &mut writer,
        &Request::Subscribe {
            sub: SUB,
            prefixes: values::PREFIXES.iter().map(|p| p.to_string()).collect(),
            period_ms: PERIOD_MS,
        },
    )
    .await?;
    send(&mut writer, &Request::Commands).await?;

    let (frames, mut incoming) = mpsc::unbounded_channel();
    let reading = tokio::spawn(async move {
        loop {
            let frame = read(&mut reader).await;
            let stop = frame.is_err();
            if frames.send(frame).is_err() || stop {
                return;
            }
        }
    });
    let ended = follow(
        state,
        telemetry,
        values,
        tank_l,
        &mut writer,
        &mut incoming,
        ap,
        out,
    )
    .await;
    reading.abort();
    ended
}

#[allow(clippy::too_many_arguments)]
async fn follow(
    state: &mut watch::Receiver<State>,
    telemetry: &mpsc::UnboundedSender<Value>,
    values: &mut Values,
    tank_l: f64,
    writer: &mut OwnedWriteHalf,
    incoming: &mut mpsc::UnboundedReceiver<io::Result<Response>>,
    ap: &mut Ap<impl Service>,
    out: &Outlets,
) -> io::Result<Ended> {
    let mut sent = Sent::default();
    let mut ui_path = out.ui_path.clone();
    let mut ui_live = true;
    loop {
        let path = ui_path.borrow_and_update().clone();
        let updates = sent.updates(&state.borrow_and_update(), &path);
        for update in updates {
            send(
                writer,
                &Request::Bridge {
                    update: Box::new(update),
                },
            )
            .await?;
        }
        let overdue = ap.deadline();
        tokio::select! {
            changed = state.changed() => {
                if changed.is_err() {
                    return Ok(Ended::StateGone);
                }
            }
            changed = ui_path.changed(), if ui_live => ui_live = changed.is_ok(),
            () = tokio::time::sleep_until(overdue.unwrap_or_else(far)), if overdue.is_some() => {
                eprintln!("[carpi] hotspot suspended too long, bringing it back");
                if let Err(e) = ap.resume().await {
                    eprintln!("[carpi] hotspot: {e}");
                }
            }
            frame = incoming.recv() => match frame {
                Some(Ok(Response::Update { sub: SUB, items })) => {
                    values.take(items);
                    if let Some(patch) = values.patch(tank_l) {
                        let _ = telemetry.send(patch);
                    }
                }
                Some(Ok(Response::Command { id, command })) => {
                    let error = match command {
                        Command::Input { action } => {
                            let now = state.borrow();
                            act(out, &now, action).err()
                        }
                        // The UI shows it over whatever is on screen.
                        Command::Toast { text, level } => {
                            let _ = out.open_page.send(livi_carpi_ui::toast_path(&text, level));
                            None
                        }
                        other => obey(ap, other).await.err(),
                    };
                    send(writer, &Request::CommandDone { id, error }).await?;
                }
                Some(Ok(Response::Error { message })) => eprintln!("[carpi] carpid: {message}"),
                Some(Ok(_)) => {}
                Some(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(Ended::Closed);
                }
                Some(Err(e)) => return Err(e),
                None => return Ok(Ended::Closed),
            },
        }
    }
}

async fn obey(ap: &mut Ap<impl Service>, command: Command) -> Result<(), String> {
    match command {
        Command::Ap {
            ap: ApAction::Suspend,
        } => ap.suspend().await,
        Command::Ap {
            ap: ApAction::Resume,
        } => ap.resume().await,
        Command::Input { .. } | Command::Toast { .. } => Ok(()),
    }
}

/// LIVI's debug switch: the config's debug logging or `DEBUG=1`.
fn debug(state: &State) -> bool {
    state.config.debug_logging || std::env::var("DEBUG").is_ok_and(|v| v == "1")
}

/// One input action: a page of ours, or the projection session's. Without a session it's
/// dropped, which isn't an error: carpid routes keys by focus, and a session can end any time.
fn act(out: &Outlets, state: &State, action: InputAction) -> Result<(), String> {
    if let InputAction::OpenPage { page } = &action {
        let path = input::page_path(page).ok_or_else(|| format!("no page {page:?}"))?;
        let _ = out.open_page.send(path);
        return Ok(());
    }
    if state.sessions.active.is_none() {
        if debug(state) {
            println!("[carpi] input {action:?} dropped: no projection session");
        }
        return Ok(());
    }
    if input::shows_projection(&action) && state.front.main != Front::Projection {
        let _ = out.open_page.send("/projection".into());
    }
    (out.input)(action);
    Ok(())
}

fn far() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(24 * 3600)
}

async fn send(writer: &mut OwnedWriteHalf, msg: &Request) -> io::Result<()> {
    writer.write_all(&carpi_proto::encode(msg)).await
}

async fn read(reader: &mut OwnedReadHalf) -> io::Result<Response> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix).await?;
    let len = carpi_proto::body_len(prefix).map_err(io::Error::other)?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    carpi_proto::decode(&body).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use carpi_proto::{Reading, Value as CarValue};
    use tokio::net::UnixListener;

    use super::*;
    use crate::ap::tests::Fake;

    async fn reply(stream: &mut UnixStream, msg: &Response) {
        stream.write_all(&carpi_proto::encode(msg)).await.unwrap();
    }

    async fn request(stream: &mut UnixStream) -> Request {
        let mut prefix = [0u8; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        let mut body = vec![0u8; carpi_proto::body_len(prefix).unwrap()];
        stream.read_exact(&mut body).await.unwrap();
        carpi_proto::decode(&body).unwrap()
    }

    #[test]
    fn the_bridge_can_run_on_any_core_thread() {
        fn send<T: Send + 'static>(_: T) {}
        let (_state, rx) = watch::channel(State::default());
        let (telemetry, _pushed) = mpsc::unbounded_channel();
        let (ready, _) = watch::channel(true);
        send(run(Hooks {
            state: rx,
            telemetry,
            ap_ready: ready,
            ui_path: watch::channel(String::new()).1,
            open_page: broadcast::channel(1).0,
            input: Box::new(|_| {}),
        }));
    }

    fn outlets() -> (
        Outlets,
        broadcast::Receiver<String>,
        std::sync::mpsc::Receiver<InputAction>,
    ) {
        let (open_page, pages) = broadcast::channel(4);
        let (tx, given) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let out = Outlets {
            ui_path: watch::channel(String::new()).1,
            open_page,
            input: Box::new(move |a| {
                let _ = tx.lock().unwrap().send(a);
            }),
        };
        (out, pages, given)
    }

    #[test]
    fn input_goes_to_the_session_or_a_page() {
        use livi_core_proto::state::{Protocol, Sessions};

        let (out, mut pages, given) = outlets();
        let mut state = State::default();
        assert_eq!(act(&out, &state, InputAction::Next), Ok(()));
        assert!(given.try_recv().is_err());

        state.sessions = Sessions {
            active: Some(Protocol::Carplay),
            ..Default::default()
        };
        act(&out, &state, InputAction::Next).unwrap();
        assert_eq!(given.try_recv(), Ok(InputAction::Next));
        assert!(pages.try_recv().is_err());
        act(&out, &state, InputAction::OpenNav).unwrap();
        assert_eq!(pages.try_recv().as_deref(), Ok("/projection"));
        assert_eq!(given.try_recv(), Ok(InputAction::OpenNav));

        let page = |p: &str| InputAction::OpenPage { page: p.into() };
        act(&out, &State::default(), page("diagnostics")).unwrap();
        assert_eq!(pages.try_recv().as_deref(), Ok("/carpi/diagnostics"));
        assert!(act(&out, &state, page("nope")).is_err());
        assert!(given.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_session_says_hello_subscribes_and_bridges_both_ways() {
        let dir = std::env::temp_dir().join(format!("livi-carpi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("carpid.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let (state_tx, mut state) = watch::channel(State::default());
        let (telemetry, mut pushed) = mpsc::unbounded_channel();
        let hotspot = Fake::default();
        *hotspot.on.lock().unwrap() = true;
        let (out, mut pages, _given) = outlets();
        let client = {
            let path = path.clone();
            let hotspot = hotspot.clone();
            tokio::spawn(async move {
                let mut values = Values::default();
                let mut connected = false;
                let mut ap = Ap::new(hotspot, None);
                session(
                    &path,
                    &mut state,
                    &telemetry,
                    &mut values,
                    55.0,
                    &mut connected,
                    &mut ap,
                    &out,
                )
                .await
            })
        };
        let (mut daemon, _) = listener.accept().await.unwrap();
        assert!(matches!(
            request(&mut daemon).await,
            Request::Hello {
                version: PROTOCOL_VERSION,
                ..
            }
        ));
        reply(
            &mut daemon,
            &Response::Hello {
                version: PROTOCOL_VERSION,
                daemon: "carpid".into(),
            },
        )
        .await;
        assert!(matches!(
            request(&mut daemon).await,
            Request::Subscribe { sub: SUB, .. }
        ));
        assert_eq!(request(&mut daemon).await, Request::Commands);
        // Every bridge section once.
        for _ in 0..8 {
            assert!(matches!(request(&mut daemon).await, Request::Bridge { .. }));
        }
        reply(
            &mut daemon,
            &Response::Update {
                sub: SUB,
                items: vec![Reading {
                    name: "vehicle.speed_kmh".into(),
                    value: Some(CarValue::Num(42.0)),
                    age_ms: 0,
                }],
            },
        )
        .await;
        assert_eq!(
            pushed.recv().await,
            Some(serde_json::json!({ "speedKph": 42.0 }))
        );
        for (id, action, on) in [(1, ApAction::Suspend, false), (2, ApAction::Resume, true)] {
            reply(
                &mut daemon,
                &Response::Command {
                    id,
                    command: Command::Ap { ap: action },
                },
            )
            .await;
            assert_eq!(
                request(&mut daemon).await,
                Request::CommandDone { id, error: None }
            );
            assert_eq!(*hotspot.on.lock().unwrap(), on);
        }
        reply(
            &mut daemon,
            &Response::Command {
                id: 3,
                command: Command::Toast {
                    text: "Capture 2026-11-08 #03".into(),
                    level: carpi_proto::ToastLevel::Info,
                },
            },
        )
        .await;
        assert_eq!(
            request(&mut daemon).await,
            Request::CommandDone { id: 3, error: None }
        );
        assert_eq!(
            pages
                .try_recv()
                .ok()
                .and_then(|p| livi_carpi_ui::toast_of(&p)),
            Some((
                "Capture 2026-11-08 #03".into(),
                carpi_proto::ToastLevel::Info
            ))
        );
        drop(daemon);
        assert!(matches!(client.await.unwrap(), Ok(Ended::Closed)));
        drop(state_tx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
