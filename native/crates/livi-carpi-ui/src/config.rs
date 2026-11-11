// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pages' own settings: the Data page's pages and the picker's recent values.
//!
//! Kept in `~/.config/LIVI/carpi-ui.json` for now (`$XDG_CONFIG_HOME/LIVI` when set). carpid
//! takes the layouts over with its settings API (docs/PAGES.md 2); this file goes then.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::defs::PRESETS;
use crate::panels::{Grid, Panel, Panels, Style, Widget};
use crate::slots::{Board, SlotPage, fit};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// The Data page's panels; `None` until edited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panels: Option<Panels>,
    /// The first Data page's tiles, read once into panels.
    #[serde(skip_serializing)]
    pub data: Option<Board>,
    /// Last values picked, newest first.
    pub recent: Vec<String>,
    /// Units carpid gave module-group values (`diag.01.103.2`), so widgets show them at once.
    pub units: BTreeMap<String, String>,
    /// The on-screen keyboard: knob, touch or off, and its letters.
    pub keyboard: crate::keyboard::Prefs,
    /// The Setup screen was hidden ("Done, hide this").
    pub setup_done: bool,
    /// The first version's groups, read once into pages.
    #[serde(skip_serializing)]
    pub group: String,
    #[serde(skip_serializing)]
    pub groups: BTreeMap<String, Vec<String>>,
}

impl Saved {
    /// The Data page's panels: saved, or the old pages as panels of numbers, then Scratch.
    pub fn panels(&self) -> Panels {
        if let Some(p) = &self.panels {
            return p.clone().fixed();
        }
        if self.data.is_none() && self.groups.is_empty() {
            return Panels::default().fixed();
        }
        let board = self.board();
        let panels = board
            .pages
            .iter()
            .map(|pg| {
                let values: Vec<&String> = pg.shown().iter().flatten().collect();
                let grid = Grid::fit(values.len());
                let (cols, _) = grid.size();
                let widgets = values
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        Widget::new(
                            (i % cols as usize) as u8,
                            (i / cols as usize) as u8,
                            Style::Number,
                            &[v.as_str()],
                        )
                    })
                    .collect();
                Panel::new(&pg.name, grid, widgets)
            })
            .collect();
        Panels {
            panels,
            shown: board.shown,
        }
        .fixed()
    }

    /// The Data page's pages: saved, from the first version's groups, or today's groups.
    pub fn board(&self) -> Board {
        if let Some(b) = &self.data {
            return b.clone().fixed();
        }
        if self.groups.is_empty() {
            return Board::data();
        }
        let pages = PRESETS
            .iter()
            .map(|p| {
                let values: Vec<&str> = match self.groups.get(p.id) {
                    Some(v) => v.iter().map(String::as_str).collect(),
                    None => p.values.to_vec(),
                };
                SlotPage::new(p.name, fit(values.len()), &values)
            })
            .collect();
        let shown = PRESETS.iter().position(|p| p.id == self.group).unwrap_or(0);
        Board { pages, shown }.fixed()
    }
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("LIVI").join("carpi-ui.json"))
}

/// What's in `path`, or the defaults when it's missing or broken.
pub fn load(path: &Path) -> Saved {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            eprintln!(
                "[carpi-ui] {} is broken, using defaults: {e}",
                path.display()
            );
            Saved::default()
        }),
        Err(_) => Saved::default(),
    }
}

/// Writes `saved` to `path` through a temporary file, so a power cut leaves the old one.
pub fn save(path: &Path, saved: &Saved) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(saved).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_versions_groups_become_pages() {
        let s: Saved = serde_json::from_str(
            r#"{"group":"body","groups":{"engine":["engine.rpm","engine.boost.actual"]}}"#,
        )
        .unwrap();
        let b = s.board();
        assert_eq!(b.shown, 2);
        assert_eq!(b.pages[0].size, 4);
        assert_eq!(b.pages[0].slots[1].as_deref(), Some("engine.boost.actual"));
        assert_eq!(b.pages[2].slots[0].as_deref(), Some("body.door_fl_open"));
        assert_eq!(Saved::default().board(), Board::data());
        // The old pages become panels of numbers, Scratch last.
        let p = s.panels();
        assert_eq!(p.shown, 2);
        assert_eq!(p.panels[0].widgets[1].values, ["engine.boost.actual"]);
        assert_eq!(p.panels[0].grid, Grid::G2x1);
        assert!(p.panels.last().unwrap().scratch);
    }

    #[test]
    fn round_trips_and_survives_a_broken_file() {
        let dir = std::env::temp_dir().join(format!("carpi-ui-{}", std::process::id()));
        let file = dir.join("LIVI").join("carpi-ui.json");
        let b = Panels {
            shown: 3,
            ..Panels::default()
        };
        let s = Saved {
            panels: Some(b),
            recent: vec!["engine.rpm".into()],
            ..Saved::default()
        };
        save(&file, &s).unwrap();
        assert_eq!(load(&file), s);
        std::fs::write(&file, b"{nope").unwrap();
        assert_eq!(load(&file), Saved::default());
        assert_eq!(load(&dir.join("missing.json")), Saved::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
