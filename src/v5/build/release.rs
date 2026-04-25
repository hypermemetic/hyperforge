//! `build::release` — version bump logic.
//!
//! Pure string manipulation; no I/O. `bump_version_in_cargo_toml`
//! returns the new file contents + old/new version for the caller to
//! write + commit.

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum BumpError {
    #[error("unknown bump kind: {0}")]
    UnknownKind(String),
    #[error("version is not semver-shaped: {0}")]
    NotSemver(String),
    #[error("manifest missing [package].version")]
    MissingVersion,
    #[error("io error: {0}")]
    Io(String),
    #[error("toml edit error: {0}")]
    Toml(String),
}

impl BumpError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnknownKind(_) => "unknown_bump_kind",
            Self::NotSemver(_) => "not_semver",
            Self::MissingVersion => "missing_version",
            Self::Io(_) => "io",
            Self::Toml(_) => "manifest_parse_error",
        }
    }
}

/// Parse a `MAJOR.MINOR.PATCH[-pre]` triple. Ignores any build/pre
/// suffix on the emitted new version — the bump resets the suffix.
fn parse_semver(v: &str) -> Result<(u64, u64, u64), BumpError> {
    let core = v.split(['-', '+']).next().unwrap_or(v);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return Err(BumpError::NotSemver(v.to_string()));
    }
    let parse = |s: &str| s.parse::<u64>().map_err(|_| BumpError::NotSemver(v.to_string()));
    Ok((parse(parts[0])?, parse(parts[1])?, parse(parts[2])?))
}

#[must_use]
pub fn apply_bump(current: (u64, u64, u64), kind: &str) -> Option<(u64, u64, u64)> {
    match kind {
        "major" => Some((current.0 + 1, 0, 0)),
        "minor" => Some((current.0, current.1 + 1, 0)),
        "patch" => Some((current.0, current.1, current.2 + 1)),
        _ => None,
    }
}

/// Edit the `version` in `[package]` of a Cargo.toml. Preserves
/// formatting via `toml_edit`. Returns `(old, new, new_text)`.
pub fn bump_cargo_toml(
    text: &str,
    kind_or_target: &str,
) -> Result<(String, String, String), BumpError> {
    let mut doc: toml_edit::DocumentMut = text.parse()
        .map_err(|e: toml_edit::TomlError| BumpError::Toml(e.to_string()))?;
    let pkg = doc.get_mut("package")
        .and_then(|t| t.as_table_mut())
        .ok_or(BumpError::MissingVersion)?;
    let old = pkg.get("version")
        .and_then(|v| v.as_str())
        .ok_or(BumpError::MissingVersion)?
        .to_string();
    let new = if ["major", "minor", "patch"].contains(&kind_or_target) {
        let parsed = parse_semver(&old)?;
        let nb = apply_bump(parsed, kind_or_target)
            .ok_or_else(|| BumpError::UnknownKind(kind_or_target.to_string()))?;
        format!("{}.{}.{}", nb.0, nb.1, nb.2)
    } else {
        // Treat as exact target. Validate shape.
        parse_semver(kind_or_target)?;
        kind_or_target.to_string()
    };
    pkg["version"] = toml_edit::value(new.clone());
    Ok((old, new, doc.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_patch_works() {
        let raw = "[package]\nname = \"foo\"\nversion = \"0.1.2\"\n";
        let (old, new, out) = bump_cargo_toml(raw, "patch").unwrap();
        assert_eq!(old, "0.1.2");
        assert_eq!(new, "0.1.3");
        assert!(out.contains("version = \"0.1.3\""));
    }

    #[test]
    fn bump_minor_resets_patch() {
        let raw = "[package]\nversion = \"1.4.7\"\n";
        let (_, new, _) = bump_cargo_toml(raw, "minor").unwrap();
        assert_eq!(new, "1.5.0");
    }

    #[test]
    fn bump_exact_target() {
        let raw = "[package]\nversion = \"0.1.0\"\n";
        let (_, new, _) = bump_cargo_toml(raw, "2.0.0").unwrap();
        assert_eq!(new, "2.0.0");
    }

    #[test]
    fn rejects_non_semver() {
        let raw = "[package]\nversion = \"not-a-version\"\n";
        assert!(bump_cargo_toml(raw, "patch").is_err());
    }
}
