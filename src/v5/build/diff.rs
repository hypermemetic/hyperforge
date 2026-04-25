//! `build::diff` — package_diff between two sets of manifests.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::manifest::PackageManifest;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackageChange {
    Added { name: String, version: String },
    Removed { name: String, version: String },
    VersionChanged { name: String, from: String, to: String },
}

/// Diff two parallel lists of manifests keyed by `name`.
pub fn diff(from: &[PackageManifest], to: &[PackageManifest]) -> Vec<PackageChange> {
    let mut from_map: BTreeMap<&str, &str> = BTreeMap::new();
    for m in from {
        from_map.insert(&m.name, &m.version);
    }
    let mut to_map: BTreeMap<&str, &str> = BTreeMap::new();
    for m in to {
        to_map.insert(&m.name, &m.version);
    }
    let mut out: Vec<PackageChange> = Vec::new();
    for (name, to_ver) in &to_map {
        match from_map.get(name) {
            None => out.push(PackageChange::Added {
                name: (*name).to_string(),
                version: (*to_ver).to_string(),
            }),
            Some(from_ver) if from_ver != to_ver => out.push(PackageChange::VersionChanged {
                name: (*name).to_string(),
                from: (*from_ver).to_string(),
                to: (*to_ver).to_string(),
            }),
            _ => {}
        }
    }
    for (name, from_ver) in &from_map {
        if !to_map.contains_key(name) {
            out.push(PackageChange::Removed {
                name: (*name).to_string(),
                version: (*from_ver).to_string(),
            });
        }
    }
    out
}
