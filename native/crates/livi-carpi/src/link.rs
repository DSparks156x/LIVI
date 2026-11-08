// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection to carpid, kept up for as long as the core runs.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use carpi_proto::{ApAction, Command, PROTOCOL_VERSION, Request, Response};
use livi_core_proto::state::State;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, watch};

use crate::ap::{Ap, Service, Systemd};
use crate::bridge::Sent;
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

/// Runs until the core's state goes away. `telemetry` is the core's own telemetry input;
/// `ap_ready` is the core's switch that holds paging while the hotspot is down.
pub async fn run(
    mut state: watch::Receiver<State>,
    telemetry: mpsc::UnboundedSender<Value>,
    ap_ready: watch::Sender<bool>,
) {
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

async fn session(
    path: &Path,
    state: &mut watch::Receiver<State>,
    telemetry: &mpsc::UnboundedSender<Value>,
    values: &mut Values,
    tank_l: f64,
    connected: &mut bool,
    ap: &mut Ap<impl Service>,
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
    )
    .await;
    reading.abort();
    ended
}

async fn follow(
    state: &mut watch::Receiver<State>,
    telemetry: &mpsc::UnboundedSender<Value>,
    values: &mut Values,
    tank_l: f64,
    writer: &mut OwnedWriteHalf,
    incoming: &mut mpsc::UnboundedReceiver<io::Result<Response>>,
    ap: &mut Ap<impl Service>,
) -> io::Result<Ended> {
    let mut sent = Sent::default();
    loop {
        let updates = sent.updates(&state.borrow_and_update());
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
                    let error = obey(ap, command).await.err();
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
    }
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
        send(run(rx, telemetry, ready));
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
        for _ in 0..7 {
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
        drop(daemon);
        assert!(matches!(client.await.unwrap(), Ok(Ended::Closed)));
        drop(state_tx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
