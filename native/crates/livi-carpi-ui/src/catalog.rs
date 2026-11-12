// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! carpid's value catalog (`Request::Catalog`): labels, units, the picker's categories and
//! order, and which values cost diag polling. Fetched once a connection; until it comes (or with
//! an old carpid) the pickers use [`crate::defs`]. The bridge values and the light model's
//! derived values aren't in it, so those keep coming from `defs`.

use std::sync::RwLock;

use carpi_proto::{CatalogEntry, ValueSource};

use crate::defs::{CATEGORIES, Category};

struct Catalog {
    cats: Vec<&'static Category>,
    /// With the category's id in place of its label.
    values: Vec<CatalogEntry>,
}

#[cfg(not(test))]
static CATALOG: RwLock<Option<Catalog>> = RwLock::new(None);

// Per test thread, so tests that set it don't reach the others.
#[cfg(test)]
thread_local! {
    static CATALOG: RwLock<Option<Catalog>> = const { RwLock::new(None) };
}

fn store<R>(f: impl FnOnce(&RwLock<Option<Catalog>>) -> R) -> R {
    #[cfg(not(test))]
    return f(&CATALOG);
    #[cfg(test)]
    CATALOG.with(f)
}

/// Our category for a catalog label: the same one in [`CATEGORIES`], or a new one. New ones are
/// leaked: a handful, once per carpid.
fn category(label: &str, subs: Vec<String>) -> &'static Category {
    if let Some(c) = CATEGORIES.iter().find(|c| c.label == label) {
        return c;
    }
    let id: &'static str = Box::leak(label.to_lowercase().into_boxed_str());
    let label: &'static str = Box::leak(label.to_string().into_boxed_str());
    let subs: Vec<&'static str> = subs
        .into_iter()
        .map(|s| &*Box::leak(s.into_boxed_str()))
        .collect();
    Box::leak(Box::new(Category {
        id,
        label,
        subs: Box::leak(subs.into_boxed_slice()),
    }))
}

/// Takes carpid's catalog. Categories in its order; ours it doesn't name (Other) go last.
pub fn set(categories: Vec<String>, mut values: Vec<CatalogEntry>) {
    let mut cats: Vec<&'static Category> = Vec::new();
    for label in &categories {
        let mut subs: Vec<String> = Vec::new();
        for v in values.iter().filter(|v| v.category == *label) {
            if !subs.contains(&v.sub) {
                subs.push(v.sub.clone());
            }
        }
        let c = category(label, subs);
        if !cats.iter().any(|x| x.id == c.id) {
            cats.push(c);
        }
    }
    for c in CATEGORIES {
        if !cats.iter().any(|x| x.id == c.id) {
            cats.push(c);
        }
    }
    for v in &mut values {
        if let Some(c) = cats.iter().find(|c| c.label == v.category) {
            v.category = c.id.to_string();
        }
    }
    store(|s| {
        if let Ok(mut c) = s.write() {
            *c = Some(Catalog { cats, values });
        }
    });
}

/// Forgets it (tests).
pub fn clear() {
    store(|s| {
        if let Ok(mut c) = s.write() {
            *c = None;
        }
    });
}

fn with<R>(f: impl FnOnce(&Catalog) -> R) -> Option<R> {
    store(|s| s.read().ok()?.as_ref().map(f))
}

/// The picker's categories, when the catalog is in.
pub fn categories() -> Option<Vec<&'static Category>> {
    with(|c| c.cats.clone())
}

/// A value's entry: label, unit, category id, sub-group, source.
pub fn entry(name: &str) -> Option<CatalogEntry> {
    with(|c| c.values.iter().find(|v| v.name == name).cloned()).flatten()
}

/// `(sub-group, name)` of a category, in the catalog's order.
pub fn members(cat: &str) -> Option<Vec<(String, String)>> {
    with(|c| {
        c.values
            .iter()
            .filter(|v| v.category == cat)
            .map(|v| (v.sub.clone(), v.name.clone()))
            .collect()
    })
}

/// Polled over diagnostics, by the catalog; `None` when it doesn't know the value.
pub fn diag(name: &str) -> Option<bool> {
    entry(name).map(|e| matches!(e.source, ValueSource::Diag { .. }))
}

#[cfg(test)]
pub(crate) fn sample() {
    let e = |name: &str, label: &str, unit: &str, cat: &str, sub: &str, diag: bool| CatalogEntry {
        name: name.into(),
        label: label.into(),
        unit: unit.into(),
        category: cat.into(),
        sub: sub.into(),
        source: if diag {
            ValueSource::Diag { modules: vec![2] }
        } else {
            ValueSource::Ican
        },
    };
    set(
        vec!["Engine".into(), "Transmission".into(), "Phone & nav".into()],
        vec![
            e(
                "engine.rpm",
                "Engine speed",
                "rpm",
                "Engine",
                "Basics",
                false,
            ),
            e(
                "trans.oil_temp_c",
                "ATF temp",
                "°C",
                "Transmission",
                "Temps",
                true,
            ),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_places_values_and_adds_categories() {
        sample();
        let cats = categories().unwrap();
        let ids: Vec<&str> = cats.iter().map(|c| c.id).collect();
        assert_eq!(&ids[..3], ["engine", "transmission", "phone"]);
        assert!(ids.contains(&"other"));
        assert_eq!(cats[1].subs, ["Temps"]);
        assert_eq!(diag("trans.oil_temp_c"), Some(true));
        assert_eq!(diag("engine.rpm"), Some(false));
        assert_eq!(diag("nav.road"), None);
        assert_eq!(
            members("transmission").unwrap(),
            [("Temps".to_string(), "trans.oil_temp_c".to_string())]
        );
        assert_eq!(entry("engine.rpm").unwrap().category, "engine");
    }
}
