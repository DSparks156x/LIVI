// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpi fork: carpid's pages in the tab bar. Data takes the dashboard's place; Diagnostics and
//! Car go before Settings.

use livi_carpi_ui::PageId;

use crate::shell::Tab;

pub fn tabs(mut tabs: Vec<Tab>) -> Vec<Tab> {
    let data = tabs.iter().position(|t| *t == Tab::Telemetry);
    tabs.retain(|t| *t != Tab::Telemetry);
    tabs.insert(data.unwrap_or(1).min(tabs.len()), Tab::Carpi(PageId::Data));
    let at = tabs
        .iter()
        .position(|t| *t == Tab::Settings)
        .unwrap_or(tabs.len());
    tabs.splice(
        at..at,
        [Tab::Carpi(PageId::Diagnostics), Tab::Carpi(PageId::Car)],
    );
    tabs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_replaces_the_dashboard() {
        let all = vec![
            Tab::Projection,
            Tab::Telemetry,
            Tab::Media,
            Tab::Camera,
            Tab::Settings,
        ];
        assert_eq!(
            tabs(all),
            vec![
                Tab::Projection,
                Tab::Carpi(PageId::Data),
                Tab::Media,
                Tab::Camera,
                Tab::Carpi(PageId::Diagnostics),
                Tab::Carpi(PageId::Car),
                Tab::Settings,
            ]
        );
        assert_eq!(
            tabs(vec![Tab::Projection, Tab::Settings]),
            vec![
                Tab::Projection,
                Tab::Carpi(PageId::Data),
                Tab::Carpi(PageId::Diagnostics),
                Tab::Carpi(PageId::Car),
                Tab::Settings,
            ]
        );
    }
}
