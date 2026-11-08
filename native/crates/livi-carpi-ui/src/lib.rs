// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpid's pages in LIVI: Data, Diagnostics and Car (with Light and Logging), and carpid's
//! toasts.
//!
//! The pages are Slint (`ui/carpi.slint`), drawn with LIVI's own rows, header and theme, and
//! focused the way LIVI's settings are. This crate holds everything that isn't Slint glue: its own
//! connection to carpid, the values and how they read, the pages' screens as plain data, and the
//! data page's selection. livi-ui's `carpi_ui.rs` copies a [`view::View`] into the `CarpiState`
//! global and hands key presses back.
//!
//! Each page reports itself to the core as the UI path `/carpi/<name>`, so the bridge can tell
//! carpid the focus is `page:<name>` (see [`focus`]).
//!
//! A picture of a page without carpid or a core, with LIVI's own snapshot switches:
//!
//! ```text
//! CARPI_UI_FIXTURE=native/crates/livi-carpi-ui/fixtures/car.json \
//! LIVI_UI_STATE=native/crates/livi-ui/fixtures/system.json \
//! LIVI_UI_SIZE=800x480 LIVI_UI_PAGE=carpi/data LIVI_UI_SNAPSHOT=/tmp/data.png LIVI
//! ```
//!
//! `LIVI_UI_PAGE` takes `carpi/data`, `carpi/diagnostics`, `carpi/car` or `carpi/car/light`.

pub mod catalog;
pub mod client;
pub mod config;
pub mod defs;
pub mod diag;
pub mod keys;
pub mod light;
pub mod logging;
pub mod pages;
pub mod settings;
pub mod slots;
pub mod values;
pub mod view;

pub use carpi_proto::ToastLevel;
pub use client::Client;
pub use pages::{Moved, Pages};

/// One of our pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageId {
    Data,
    Diagnostics,
    Car,
}

impl PageId {
    /// In tab order.
    pub const ALL: [PageId; 3] = [PageId::Data, PageId::Diagnostics, PageId::Car];

    pub fn name(self) -> &'static str {
        match self {
            PageId::Data => "data",
            PageId::Diagnostics => "diagnostics",
            PageId::Car => "car",
        }
    }

    /// The UI path livi-ui reports to the core while the page is shown.
    pub fn path(self) -> &'static str {
        match self {
            PageId::Data => "/carpi/data",
            PageId::Diagnostics => "/carpi/diagnostics",
            PageId::Car => "/carpi/car",
        }
    }

    /// LIVI icon of the page's tab.
    pub fn icon(self) -> &'static str {
        match self {
            PageId::Data => "SpeedOutlined",
            PageId::Diagnostics => "BugReportOutlined",
            PageId::Car => "DirectionsCarOutlined",
        }
    }

    pub fn from_name(name: &str) -> Option<PageId> {
        PageId::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// The light page: a screen of the Car page, reached from it or by carpid's `OpenPage`, with a
/// focus of its own.
pub const LIGHT_PATH: &str = "/carpi/car/light";

/// carpid's focus for a UI path: `page:<name>` for one of ours, `None` for anything else.
///
/// For livi-carpi's bridge, which sees the path through the core's `ui_path`.
pub fn focus(ui_path: &str) -> Option<String> {
    if ui_path.starts_with(LIGHT_PATH) {
        return Some("page:light".into());
    }
    let name = ui_path.strip_prefix("/carpi/")?;
    let name = name.split('/').next()?;
    PageId::from_name(name).map(|p| format!("page:{}", p.name()))
}

/// LIVI's UI path for one of our pages by name, as carpid's `OpenPage` names them.
pub fn page_path(name: &str) -> Option<String> {
    if name == "light" {
        return Some(LIGHT_PATH.into());
    }
    PageId::from_name(name).map(|p| p.path().to_string())
}

/// carpid's toasts reach the UI over the core's open-page channel, as this path.
const TOAST: &str = "/carpi/toast/";

/// The path the bridge sends for a toast: `/carpi/toast/<level>/<text>`.
pub fn toast_path(text: &str, level: ToastLevel) -> String {
    let level = match level {
        ToastLevel::Info => "info",
        ToastLevel::Warn => "warn",
        ToastLevel::Error => "error",
    };
    format!("{TOAST}{level}/{text}")
}

/// The toast a path carries, if it's one.
pub fn toast_of(path: &str) -> Option<(String, ToastLevel)> {
    let (level, text) = path.strip_prefix(TOAST)?.split_once('/')?;
    let level = match level {
        "warn" => ToastLevel::Warn,
        "error" => ToastLevel::Error,
        _ => ToastLevel::Info,
    };
    Some((text.to_string(), level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_name_the_focus() {
        for p in PageId::ALL {
            assert_eq!(focus(p.path()), Some(format!("page:{}", p.name())));
        }
        assert_eq!(focus("/carpi/car/settings"), Some("page:car".into()));
        assert_eq!(focus("/media"), None);
        assert_eq!(focus("/carpi/nope"), None);
        assert_eq!(PageId::from_name("diagnostics"), Some(PageId::Diagnostics));
        assert_eq!(page_path("car").as_deref(), Some("/carpi/car"));
        assert_eq!(page_path("light").as_deref(), Some(LIGHT_PATH));
        assert_eq!(focus(LIGHT_PATH), Some("page:light".into()));
        assert_eq!(page_path("nope"), None);
    }

    #[test]
    fn toasts_ride_a_path() {
        let p = toast_path("Capture 2026-11-08 #03 / CAN", ToastLevel::Warn);
        assert_eq!(
            toast_of(&p),
            Some(("Capture 2026-11-08 #03 / CAN".into(), ToastLevel::Warn))
        );
        assert_eq!(toast_of("/carpi/data"), None);
        assert_eq!(focus(&p), None);
    }
}
