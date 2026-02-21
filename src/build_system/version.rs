//! SemVer parsing, comparison, and manifest version editing.
//!
//! Provides version parsing/bumping and format-preserving manifest editing
//! for Cargo.toml (via toml_edit) and .cabal files (line-based replace).

use crate::build_system::BuildSystemKind;
use crate::types::VersionBump;
use std::cmp::Ordering;
use std::path::Path;

/// A parsed semantic version (major.minor.patch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl SemVer {
    /// Parse a semver string like "1.2.3" or "v1.2.3".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('v').unwrap_or(s).trim();
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(Self {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
        })
    }

    /// Bump the version according to the given kind.
    pub fn bump(&self, kind: &VersionBump) -> Self {
        match kind {
            VersionBump::Patch => Self {
                major: self.major,
                minor: self.minor,
                patch: self.patch + 1,
            },
            VersionBump::Minor => Self {
                major: self.major,
                minor: self.minor + 1,
                patch: 0,
            },
            VersionBump::Major => Self {
                major: self.major + 1,
                minor: 0,
                patch: 0,
            },
        }
    }

    /// Format as "major.minor.patch".
    pub fn to_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
    }
}

/// Compare two version strings. Returns None if either fails to parse.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let va = SemVer::parse(a)?;
    let vb = SemVer::parse(b)?;
    Some(va.cmp(&vb))
}

/// Edit the version field in a Cargo.toml, preserving formatting via toml_edit.
///
/// Returns the new file content as a String.
pub fn set_cargo_version(path: &Path, new_version: &str) -> anyhow::Result<String> {
    let cargo_path = path.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;

    doc["package"]["version"] = toml_edit::value(new_version);

    let result = doc.to_string();
    std::fs::write(&cargo_path, &result)?;
    Ok(result)
}

/// Edit the version field in a .cabal file using line-based replacement.
///
/// Looks for a line starting with "version:" (case-insensitive) and replaces
/// the value portion. Returns the new file content.
pub fn set_cabal_version(path: &Path, new_version: &str) -> anyhow::Result<String> {
    let cabal_path = find_cabal_file(path)
        .ok_or_else(|| anyhow::anyhow!("No .cabal file found in {}", path.display()))?;

    let content = std::fs::read_to_string(&cabal_path)?;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let mut found = false;

    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.to_lowercase().starts_with("version:") {
            // Preserve leading whitespace
            let leading: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            *line = format!("{}version:            {}", leading, new_version);
            found = true;
            break;
        }
    }

    if !found {
        return Err(anyhow::anyhow!(
            "No version field found in {}",
            cabal_path.display()
        ));
    }

    let result = lines.join("\n") + "\n";
    std::fs::write(&cabal_path, &result)?;
    Ok(result)
}

/// Edit the version in a package.json file.
///
/// Uses serde_json to parse and rewrite while preserving the key set.
pub fn set_node_version(path: &Path, new_version: &str) -> anyhow::Result<String> {
    let pkg_path = path.join("package.json");
    let content = std::fs::read_to_string(&pkg_path)?;
    let mut value: serde_json::Value = serde_json::from_str(&content)?;

    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "version".to_string(),
            serde_json::Value::String(new_version.to_string()),
        );
    } else {
        return Err(anyhow::anyhow!("package.json is not a JSON object"));
    }

    let result = serde_json::to_string_pretty(&value)? + "\n";
    std::fs::write(&pkg_path, &result)?;
    Ok(result)
}

/// Dispatch to the appropriate manifest editor based on build system kind.
///
/// Returns the new file content.
pub fn set_package_version(
    path: &Path,
    kind: &BuildSystemKind,
    new_version: &str,
) -> anyhow::Result<String> {
    match kind {
        BuildSystemKind::Cargo => set_cargo_version(path, new_version),
        BuildSystemKind::Cabal => set_cabal_version(path, new_version),
        BuildSystemKind::Node => set_node_version(path, new_version),
        BuildSystemKind::Unknown => {
            Err(anyhow::anyhow!("Cannot set version for unknown build system"))
        }
    }
}

/// Find the first .cabal file in a directory.
fn find_cabal_file(path: &Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(path).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("cabal") {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_basic() {
        let v = SemVer::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_parse_with_v_prefix() {
        let v = SemVer::parse("v0.5.12").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 5);
        assert_eq!(v.patch, 12);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(SemVer::parse("1.2").is_none());
        assert!(SemVer::parse("abc").is_none());
        assert!(SemVer::parse("1.2.3.4").is_none());
        assert!(SemVer::parse("").is_none());
    }

    #[test]
    fn test_bump_patch() {
        let v = SemVer::parse("1.2.3").unwrap();
        let bumped = v.bump(&VersionBump::Patch);
        assert_eq!(bumped.to_string(), "1.2.4");
    }

    #[test]
    fn test_bump_minor() {
        let v = SemVer::parse("1.2.3").unwrap();
        let bumped = v.bump(&VersionBump::Minor);
        assert_eq!(bumped.to_string(), "1.3.0");
    }

    #[test]
    fn test_bump_major() {
        let v = SemVer::parse("1.2.3").unwrap();
        let bumped = v.bump(&VersionBump::Major);
        assert_eq!(bumped.to_string(), "2.0.0");
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Some(Ordering::Equal));
        assert_eq!(compare_versions("1.2.4", "1.2.3"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.2.3", "1.3.0"), Some(Ordering::Less));
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("bad", "1.2.3"), None);
    }

    #[test]
    fn test_set_cargo_version_preserves_formatting() {
        let tmp = TempDir::new().unwrap();
        let cargo_toml = r#"[package]
name = "my-crate"
version = "0.3.0"
edition = "2021"

# This comment should be preserved
[dependencies]
serde = "1"
"#;
        fs::write(tmp.path().join("Cargo.toml"), cargo_toml).unwrap();

        let result = set_cargo_version(tmp.path(), "0.3.1").unwrap();
        assert!(result.contains("version = \"0.3.1\""));
        assert!(result.contains("# This comment should be preserved"));
        assert!(result.contains("name = \"my-crate\""));
    }

    #[test]
    fn test_set_cabal_version() {
        let tmp = TempDir::new().unwrap();
        let cabal_content = "name:               my-package\nversion:            0.2.0\nbuild-type:         Simple\n";
        fs::write(tmp.path().join("my-package.cabal"), cabal_content).unwrap();

        let result = set_cabal_version(tmp.path(), "0.2.1").unwrap();
        assert!(result.contains("version:            0.2.1"));
        assert!(result.contains("name:               my-package"));
    }

    #[test]
    fn test_set_package_version_dispatch() {
        let tmp = TempDir::new().unwrap();
        let cargo_toml = "[package]\nname = \"test\"\nversion = \"1.0.0\"\n";
        fs::write(tmp.path().join("Cargo.toml"), cargo_toml).unwrap();

        set_package_version(tmp.path(), &BuildSystemKind::Cargo, "1.0.1").unwrap();
        let content = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(content.contains("version = \"1.0.1\""));
    }

    #[test]
    fn test_ordering() {
        let v1 = SemVer::parse("0.1.0").unwrap();
        let v2 = SemVer::parse("0.2.0").unwrap();
        let v3 = SemVer::parse("1.0.0").unwrap();
        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 > v1);
    }
}
