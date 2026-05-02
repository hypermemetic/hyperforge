//! Registry version queries for crates.io, Hackage, and npm.
//!
//! Lightweight HTTP queries that return the latest published version
//! for a given package name. No auth required (public read endpoints).

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum RegistryError {
    #[error("http error querying {registry} for {name}: {message}")]
    Http {
        registry: String,
        name: String,
        message: String,
    },
    #[error("unexpected response from {registry} for {name}: {message}")]
    Parse {
        registry: String,
        name: String,
        message: String,
    },
}

impl RegistryError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Http { .. } => "registry_http",
            Self::Parse { .. } => "registry_parse",
        }
    }
}

/// Query crates.io for the latest stable version of a crate.
pub async fn query_crates_io(name: &str) -> Result<Option<String>, RegistryError> {
    let client = reqwest::Client::builder()
        .user_agent("hyperforge/5.0 (https://github.com/juggernautlabs/hyperforge)")
        .build()
        .map_err(|e| RegistryError::Http {
            registry: "crates_io".into(),
            name: name.into(),
            message: e.to_string(),
        })?;

    let url = format!("https://crates.io/api/v1/crates/{name}");
    let resp = client.get(&url).send().await.map_err(|e| RegistryError::Http {
        registry: "crates_io".into(),
        name: name.into(),
        message: e.to_string(),
    })?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !resp.status().is_success() {
        return Err(RegistryError::Http {
            registry: "crates_io".into(),
            name: name.into(),
            message: format!("status {}", resp.status()),
        });
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| RegistryError::Parse {
        registry: "crates_io".into(),
        name: name.into(),
        message: e.to_string(),
    })?;

    let version = body
        .get("crate")
        .and_then(|c| {
            c.get("max_stable_version")
                .or_else(|| c.get("max_version"))
        })
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(version)
}

/// Query Hackage for the latest version of a package.
pub async fn query_hackage(name: &str) -> Result<Option<String>, RegistryError> {
    let client = reqwest::Client::builder()
        .user_agent("hyperforge/5.0")
        .build()
        .map_err(|e| RegistryError::Http {
            registry: "hackage".into(),
            name: name.into(),
            message: e.to_string(),
        })?;

    let url = format!("https://hackage.haskell.org/package/{name}/preferred");
    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| RegistryError::Http {
            registry: "hackage".into(),
            name: name.into(),
            message: e.to_string(),
        })?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !resp.status().is_success() {
        return Err(RegistryError::Http {
            registry: "hackage".into(),
            name: name.into(),
            message: format!("status {}", resp.status()),
        });
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| RegistryError::Parse {
        registry: "hackage".into(),
        name: name.into(),
        message: e.to_string(),
    })?;

    // Hackage response: { "normal-version": ["1.2.3", "1.2.2", ...], ... }
    let version = body
        .get("normal-version")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(version)
}

/// Query npm for the latest version of a package.
pub async fn query_npm(name: &str) -> Result<Option<String>, RegistryError> {
    let client = reqwest::Client::builder()
        .user_agent("hyperforge/5.0")
        .build()
        .map_err(|e| RegistryError::Http {
            registry: "npm".into(),
            name: name.into(),
            message: e.to_string(),
        })?;

    let url = format!("https://registry.npmjs.org/{name}");
    let resp = client.get(&url).send().await.map_err(|e| RegistryError::Http {
        registry: "npm".into(),
        name: name.into(),
        message: e.to_string(),
    })?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !resp.status().is_success() {
        return Err(RegistryError::Http {
            registry: "npm".into(),
            name: name.into(),
            message: format!("status {}", resp.status()),
        });
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| RegistryError::Parse {
        registry: "npm".into(),
        name: name.into(),
        message: e.to_string(),
    })?;

    let version = body
        .get("dist-tags")
        .and_then(|dt| dt.get("latest"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(version)
}

/// Compare two semver-ish version strings.
/// Returns `Ordering::Greater` if `a > b`, etc.
/// Splits on `.`, compares each segment numerically left-to-right.
/// Non-numeric segments compared lexicographically.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();

    let max_len = a_parts.len().max(b_parts.len());
    for i in 0..max_len {
        let ap = a_parts.get(i).copied().unwrap_or("0");
        let bp = b_parts.get(i).copied().unwrap_or("0");

        // Try numeric comparison first.
        match (ap.parse::<u64>(), bp.parse::<u64>()) {
            (Ok(an), Ok(bn)) => {
                let ord = an.cmp(&bn);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            _ => {
                let ord = ap.cmp(bp);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
        }
    }
    std::cmp::Ordering::Equal
}

/// Determine the diff status between local and published versions.
pub fn diff_status(local: &str, published: Option<&str>) -> &'static str {
    match published {
        None => "unpublished",
        Some(pub_ver) => match compare_versions(local, pub_ver) {
            std::cmp::Ordering::Greater => "ahead",
            std::cmp::Ordering::Less => "stale",
            std::cmp::Ordering::Equal => "up_to_date",
        },
    }
}

/// Detect the build system kind and registry name for a directory.
/// Returns `(kind, registry)` or `None` if no known manifest found.
pub fn detect_build_system(dir: &std::path::Path) -> Option<(&'static str, &'static str)> {
    if dir.join("Cargo.toml").is_file() {
        return Some(("cargo", "crates_io"));
    }
    // Check for .cabal files.
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("cabal") && p.is_file() {
                return Some(("cabal", "hackage"));
            }
        }
    }
    if dir.join("package.json").is_file() {
        return Some(("node", "npm"));
    }
    None
}

/// Parse the local package name and version from the detected build system.
/// Returns `(name, version)`.
pub fn parse_local_version(
    dir: &std::path::Path,
    build_system: &str,
) -> Option<(String, String)> {
    match build_system {
        "cargo" => {
            let cargo = dir.join("Cargo.toml");
            let raw = std::fs::read_to_string(&cargo).ok()?;
            let v: toml::Value = toml::from_str(&raw).ok()?;
            let pkg = v.get("package")?.as_table()?;
            let name = pkg.get("name")?.as_str()?.to_string();
            let version = pkg.get("version")?.as_str()?.to_string();
            Some((name, version))
        }
        "cabal" => {
            // Find .cabal file and parse name/version fields.
            let entries = std::fs::read_dir(dir).ok()?;
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("cabal") && p.is_file() {
                    let content = std::fs::read_to_string(&p).ok()?;
                    let name = parse_cabal_field(&content, "name")?;
                    let version = parse_cabal_field(&content, "version")?;
                    return Some((name, version));
                }
            }
            None
        }
        "node" => {
            let pkg = dir.join("package.json");
            let raw = std::fs::read_to_string(&pkg).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            let name = v.get("name")?.as_str()?.to_string();
            let version = v.get("version")?.as_str()?.to_string();
            Some((name, version))
        }
        _ => None,
    }
}

/// Parse a top-level field from cabal file content (e.g., `name:`, `version:`).
fn parse_cabal_field(content: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}:");
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.to_lowercase().starts_with(&prefix) {
            let value = trimmed[prefix.len()..].trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Query the appropriate registry for a package version.
pub async fn query_registry(
    package_name: &str,
    registry: &str,
) -> Result<Option<String>, RegistryError> {
    match registry {
        "crates_io" => query_crates_io(package_name).await,
        "hackage" => query_hackage(package_name).await,
        "npm" => query_npm(package_name).await,
        other => Err(RegistryError::Parse {
            registry: other.into(),
            name: package_name.into(),
            message: format!("unknown registry: {other}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_versions_equal() {
        assert_eq!(compare_versions("1.2.3", "1.2.3"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_compare_versions_ahead() {
        assert_eq!(compare_versions("0.5.0", "0.4.0"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "0.9.9"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("2.0.0", "1.99.99"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_compare_versions_behind() {
        assert_eq!(compare_versions("0.4.0", "0.5.0"), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_compare_versions_different_lengths() {
        assert_eq!(compare_versions("1.2", "1.2.0"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("1.2.1", "1.2"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_diff_status_unpublished() {
        assert_eq!(diff_status("0.1.0", None), "unpublished");
    }

    #[test]
    fn test_diff_status_ahead() {
        assert_eq!(diff_status("0.5.0", Some("0.4.0")), "ahead");
    }

    #[test]
    fn test_diff_status_stale() {
        assert_eq!(diff_status("0.3.0", Some("0.4.0")), "stale");
    }

    #[test]
    fn test_diff_status_up_to_date() {
        assert_eq!(diff_status("1.0.0", Some("1.0.0")), "up_to_date");
    }

    #[test]
    fn test_parse_cabal_field_name() {
        let content = "name:          plexus-synapse\nversion:       0.2.0.1\n";
        assert_eq!(parse_cabal_field(content, "name"), Some("plexus-synapse".into()));
        assert_eq!(parse_cabal_field(content, "version"), Some("0.2.0.1".into()));
    }
}
