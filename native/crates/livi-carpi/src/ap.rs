// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The hotspot, suspended while carpid fetches an update over a normal Wi-Fi.
//!
//! Suspend stops `livi-wifi-ap.service`, LIVI's own AP unit. Its `ExecStopPost`
//! (`livi-helperd --wifi-ap-teardown`) hands the interface back to NetworkManager, which
//! joins a known network. Resume starts the unit again, only if it ran before. Both go through
//! the sudo rule LIVI installs for that unit. While the AP is down, paging phones waits, the same
//! as during LIVI's own AP restarts.
//!
//! The AP comes back on resume, when carpid's connection drops, and after
//! [`carpi_proto::AP_SUSPEND_MAX_S`] at the latest.

use std::future::Future;
use std::time::Duration;

use livi_core_proto::state::State;
use tokio::sync::watch;
use tokio::time::Instant;

pub const SERVICE: &str = "livi-wifi-ap.service";
const ON_AIR_WAIT: Duration = Duration::from_secs(30);
const ON_AIR_POLL: Duration = Duration::from_millis(250);

/// The AP unit.
pub trait Service: Send {
    fn active(&mut self) -> impl Future<Output = bool> + Send;
    /// `true` when it worked.
    fn set(&mut self, on: bool) -> impl Future<Output = bool> + Send;
    /// Resolves once the AP is on air, or after a while.
    fn on_air(&mut self) -> impl Future<Output = ()> + Send;
}

pub struct Ap<S> {
    service: S,
    /// The core's "AP is ready" switch, which holds paging while false.
    ready: Option<watch::Sender<bool>>,
    /// While suspended: whether the AP ran before, and since when.
    held: Option<(bool, Instant)>,
}

impl<S: Service> Ap<S> {
    pub fn new(service: S, ready: Option<watch::Sender<bool>>) -> Self {
        Ap {
            service,
            ready,
            held: None,
        }
    }

    /// When the AP comes back on its own.
    pub fn deadline(&self) -> Option<Instant> {
        self.held
            .map(|(_, since)| since + Duration::from_secs(carpi_proto::AP_SUSPEND_MAX_S))
    }

    pub async fn suspend(&mut self) -> Result<(), String> {
        if self.held.is_some() {
            return Ok(());
        }
        let was = self.service.active().await;
        if was {
            self.ready(false);
            if !self.service.set(false).await {
                self.ready(true);
                return Err(format!("stopping {SERVICE} failed"));
            }
        }
        println!(
            "[carpi] hotspot suspended for an update (was {})",
            on_off(was)
        );
        self.held = Some((was, Instant::now()));
        Ok(())
    }

    /// Puts back what ran before [`Ap::suspend`]. Nothing to do when not suspended.
    pub async fn resume(&mut self) -> Result<(), String> {
        let Some((was, _)) = self.held.take() else {
            return Ok(());
        };
        if !was {
            println!("[carpi] update done, hotspot stays off as before");
            return Ok(());
        }
        let started = self.service.set(true).await;
        if started {
            self.service.on_air().await;
        }
        self.ready(true);
        if started {
            println!("[carpi] hotspot back");
            Ok(())
        } else {
            Err(format!("starting {SERVICE} failed"))
        }
    }

    fn ready(&self, ready: bool) {
        if let Some(r) = &self.ready {
            r.send_replace(ready);
        }
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// `systemctl` through LIVI's sudo rule.
pub struct Systemd {
    /// For the AP interface in the config.
    pub state: watch::Receiver<State>,
}

fn systemctl() -> &'static str {
    [
        "/usr/bin/systemctl",
        "/bin/systemctl",
        "/usr/sbin/systemctl",
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).exists())
    .unwrap_or("/usr/bin/systemctl")
}

async fn quietly(program: &str, args: &[&str]) -> bool {
    tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .is_ok_and(|s| s.success())
}

impl Service for Systemd {
    async fn active(&mut self) -> bool {
        quietly(systemctl(), &["is-active", "--quiet", SERVICE]).await
    }

    async fn set(&mut self, on: bool) -> bool {
        let verb = if on { "start" } else { "stop" };
        // The unit stops within its TimeoutStopSec of 8 s.
        tokio::time::timeout(
            Duration::from_secs(20),
            quietly("sudo", &["-n", systemctl(), verb, SERVICE]),
        )
        .await
        .unwrap_or(false)
    }

    async fn on_air(&mut self) {
        let iface = self.state.borrow().config.wifi_interface.clone();
        let deadline = Instant::now() + ON_AIR_WAIT;
        while livi_wifi::on_air(&iface).is_none() && Instant::now() < deadline {
            tokio::time::sleep(ON_AIR_POLL).await;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    pub struct Fake {
        pub on: Arc<Mutex<bool>>,
        pub calls: Arc<Mutex<Vec<String>>>,
        pub refuse: bool,
    }

    impl Service for Fake {
        async fn active(&mut self) -> bool {
            *self.on.lock().unwrap()
        }
        async fn set(&mut self, on: bool) -> bool {
            self.calls.lock().unwrap().push(on_off(on).into());
            if self.refuse {
                return false;
            }
            *self.on.lock().unwrap() = on;
            true
        }
        async fn on_air(&mut self) {}
    }

    #[tokio::test]
    async fn resume_restores_what_ran_before() {
        let fake = Fake::default();
        *fake.on.lock().unwrap() = true;
        let (ready, readiness) = watch::channel(true);
        let mut ap = Ap::new(fake.clone(), Some(ready));
        ap.suspend().await.unwrap();
        assert!(!*fake.on.lock().unwrap());
        assert!(!*readiness.borrow());
        // A second suspend is a no-op.
        ap.suspend().await.unwrap();
        ap.resume().await.unwrap();
        assert!(*fake.on.lock().unwrap());
        assert!(*readiness.borrow());
        ap.resume().await.unwrap();
        assert_eq!(*fake.calls.lock().unwrap(), ["off", "on"]);
    }

    #[tokio::test]
    async fn an_ap_that_was_off_stays_off() {
        let fake = Fake::default();
        let mut ap = Ap::new(fake.clone(), None);
        ap.suspend().await.unwrap();
        assert!(ap.deadline().is_some());
        ap.resume().await.unwrap();
        assert!(!*fake.on.lock().unwrap());
        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_refused_stop_leaves_the_ap_ready() {
        let fake = Fake {
            refuse: true,
            ..Fake::default()
        };
        *fake.on.lock().unwrap() = true;
        let (ready, readiness) = watch::channel(true);
        let mut ap = Ap::new(fake, Some(ready));
        assert!(ap.suspend().await.is_err());
        assert!(ap.deadline().is_none());
        assert!(*readiness.borrow());
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_is_four_minutes_after_suspend() {
        let mut ap = Ap::new(Fake::default(), None);
        assert_eq!(ap.deadline(), None);
        let t = Instant::now();
        ap.suspend().await.unwrap();
        assert_eq!(ap.deadline(), Some(t + Duration::from_secs(240)));
    }
}
