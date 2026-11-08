// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pages' own settings: which values each data group shows.
//!
//! Kept in `~/.config/LIVI/carpi-ui.json` for now (`$XDG_CONFIG_HOME/LIVI` when set). carpid will
//! keep this with its config once it has `config.get/set`; this file goes then.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::defs::{GROUPS, MAX_CELLS};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// Group the data page shows.
    pub group: String,
    /// Values per group id. A group that isn't here shows its preset.
    pub groups: BTreeMap<String, Vec<String>>,
}

impl Saved {
    pub fn group(&self) -> &'static str {
        GROUPS
            .iter()
            .find(|g| g.id == self.group)
            .unwrap_or(&GROUPS[0])
            .id
    }

    /// Values of group `id`, at most [`MAX_CELLS`].
    pub fn values(&self, id: &str) -> Vec<String> {
        match self.groups.get(id) {
            Some(v) => v.iter().take(MAX_CELLS).cloned().collect(),
            None => GROUPS
                .iter()
                .find(|g| g.id == id)
                .map(|g| g.preset.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
        }
    }

    /// Adds or drops `name` in group `id`. False when the group is full.
    pub fn toggle(&mut self, id: &str, name: &str) -> bool {
        let mut v = self.values(id);
        if let Some(i) = v.iter().position(|n| n == name) {
            v.remove(i);
        } else if v.len() >= MAX_CELLS {
            return false;
        } else {
            v.push(name.to_string());
        }
        self.groups.insert(id.to_string(), v);
        true
    }

    pub fn reset(&mut self, id: &str) {
        self.groups.remove(id);
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
    fn presets_until_edited() {
        let mut s = Saved::default();
        assert_eq!(s.group(), "engine");
        assert_eq!(s.values("engine")[0], "engine.rpm");
        assert!(s.toggle("engine", "engine.rpm"));
        assert!(!s.values("engine").contains(&"engine.rpm".to_string()));
        assert!(s.toggle("engine", "engine.rpm"));
        assert_eq!(s.values("engine").last().unwrap(), "engine.rpm");
        s.reset("engine");
        assert_eq!(s.values("engine")[0], "engine.rpm");
    }

    #[test]
    fn a_full_group_takes_no_more() {
        let mut s = Saved::default();
        for i in 0..MAX_CELLS {
            s.groups
                .entry("body".into())
                .or_default()
                .push(format!("x.{i}"));
        }
        s.groups.get_mut("body").unwrap().truncate(MAX_CELLS);
        assert!(!s.toggle("body", "x.new"));
        assert!(s.toggle("body", "x.0"));
        assert!(s.toggle("body", "x.new"));
    }

    #[test]
    fn round_trips_and_survives_a_broken_file() {
        let dir = std::env::temp_dir().join(format!("carpi-ui-{}", std::process::id()));
        let file = dir.join("LIVI").join("carpi-ui.json");
        let mut s = Saved {
            group: "body".into(),
            ..Saved::default()
        };
        s.toggle("body", "lights.drl");
        save(&file, &s).unwrap();
        assert_eq!(load(&file), s);
        std::fs::write(&file, b"{nope").unwrap();
        assert_eq!(load(&file), Saved::default());
        assert_eq!(load(&dir.join("missing.json")), Saved::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
