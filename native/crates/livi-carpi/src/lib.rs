// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bridge between livi-core and carpid, the car daemon of pq35-carpi.
//!
//! carpid owns the car's CAN. This crate connects to its socket, `$CARPID_SOCKET` or
//! `$XDG_RUNTIME_DIR/carpid.sock`, and
//! - pushes what LIVI knows that the car needs, projection, guidance, turn icon, media, calls,
//!   phone and focus, each time it changes,
//! - subscribes to the car values LIVI uses and hands them to the core as telemetry, which feeds
//!   Android Auto's sensors and LIVI's own look (`light.night` sets night and dark mode).
//!
//! Without carpid it retries with a backoff and LIVI runs as without it.

#![cfg_attr(not(unix), allow(dead_code))]

mod bridge;
#[cfg(unix)]
mod link;
mod values;

#[cfg(unix)]
pub use link::run;
