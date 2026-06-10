//! `build::release` — version bump + publish command generation.
//!
//! Pure string manipulation; no I/O. `bump_cargo_toml` / `bump_cabal_file`
//! return the new file contents + old/new version for the caller to
//! write + commit. `publish_command` maps a channel name to the shell
//! command that publishes (token injected by caller).

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

// ── Cabal (.cabal file) support ─────────────────────────────────────

/// Parse the `version:` field from a `.cabal` file's text content.
/// Returns `None` if the field is absent or empty.
pub fn parse_cabal_version(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .to_lowercase()
            .strip_prefix("version:")
            .map(|_| trimmed["version:".len()..].trim())
        {
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Bump the `version:` field in a `.cabal` file.
/// Returns `(old, new, new_text)` — same contract as `bump_cargo_toml`.
pub fn bump_cabal_file(
    text: &str,
    kind_or_target: &str,
) -> Result<(String, String, String), BumpError> {
    let old = parse_cabal_version(text).ok_or(BumpError::MissingVersion)?;
    let new = if ["major", "minor", "patch"].contains(&kind_or_target) {
        let parsed = parse_semver(&old)?;
        let nb = apply_bump(parsed, kind_or_target)
            .ok_or_else(|| BumpError::UnknownKind(kind_or_target.to_string()))?;
        format!("{}.{}.{}", nb.0, nb.1, nb.2)
    } else {
        parse_semver(kind_or_target)?;
        kind_or_target.to_string()
    };
    // Replace the first `version:` line preserving whitespace style.
    let mut found = false;
    let new_text: String = text
        .lines()
        .map(|line| {
            if !found {
                let trimmed = line.trim();
                if trimmed.to_lowercase().starts_with("version:") {
                    found = true;
                    // Preserve the key prefix (indentation + casing) and
                    // spacing between the colon and value.
                    let colon_pos = line.find(':').unwrap();
                    let after_colon = &line[colon_pos + 1..];
                    let leading_spaces = after_colon.len() - after_colon.trim_start().len();
                    let padding = if leading_spaces > 0 {
                        &after_colon[..leading_spaces]
                    } else {
                        " "
                    };
                    return format!("{}{padding}{new}", &line[..=colon_pos]);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    // Preserve trailing newline if original had one.
    let new_text = if text.ends_with('\n') && !new_text.ends_with('\n') {
        new_text + "\n"
    } else {
        new_text
    };
    Ok((old, new, new_text))
}

// ── Publish command generation ──────────────────────────────────────

/// The secret key path (under `secrets://`) for a given publish channel.
/// Returns `None` for unknown channels.
#[must_use]
pub fn secret_key_for_channel(channel: &str) -> Option<&'static str> {
    match channel {
        "crates.io" => Some("cargo/token"),
        "npm" => Some("npm/token"),
        "pypi" => Some("pypi/token"),
        "hackage" => Some("hackage/token"),
        _ => None,
    }
}

/// Build the shell command string to publish a package on `channel`.
/// The resolved `token` is embedded in the command via env var or flag.
///
/// For hackage the flow is:
///   1. `cabal sdist` — build source distribution tarball
///   2. `cabal upload --publish --token=<tok> <tarball>`
///
/// The tarball path is discovered from `cabal sdist` stdout at runtime,
/// so we emit a two-step shell pipeline that captures the path.
#[must_use]
pub fn publish_command(channel: &str, token: &str) -> Option<String> {
    let escaped = shell_escape(token);
    match channel {
        // NB: no `--allow-dirty` — HF-PUBLISH-SAFETY. A dirty tree must
        // be a named hard error in the pre-flight (build::preflight),
        // never silently shipped. The `force_dirty` escape hatch skips
        // the pre-flight gate but still publishes from the committed
        // index state cargo enforces.
        "crates.io" => Some(format!(
            "CARGO_REGISTRY_TOKEN={escaped} cargo publish"
        )),
        "npm" => Some(format!("NPM_TOKEN={escaped} npm publish")),
        "pypi" => Some(format!(
            "TWINE_USERNAME=__token__ TWINE_PASSWORD={escaped} twine upload dist/*"
        )),
        "hackage" => Some(format!(
            "cabal sdist && cabal upload --publish --token={escaped} \
             \"$(cabal sdist 2>/dev/null | tail -1)\""
        )),
        _ => None,
    }
}

/// Minimal shell escaping: wrap in single quotes, escaping embedded
/// single quotes via the `'\''` idiom.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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

    // ── Cabal tests ─────────────────────────────────────────────────

    #[test]
    fn parse_cabal_version_basic() {
        let cabal = "name:           plexus-synapse\nversion:        0.2.0\nlicense:        MIT\n";
        assert_eq!(parse_cabal_version(cabal), Some("0.2.0".into()));
    }

    #[test]
    fn parse_cabal_version_compact() {
        let cabal = "name: foo\nversion:1.0.0\n";
        assert_eq!(parse_cabal_version(cabal), Some("1.0.0".into()));
    }

    #[test]
    fn parse_cabal_version_missing() {
        let cabal = "name: foo\nlicense: MIT\n";
        assert_eq!(parse_cabal_version(cabal), None);
    }

    #[test]
    fn bump_cabal_patch() {
        let cabal = "name:           synapse\nversion:        0.2.0\nlicense:        MIT\n";
        let (old, new, out) = bump_cabal_file(cabal, "patch").unwrap();
        assert_eq!(old, "0.2.0");
        assert_eq!(new, "0.2.1");
        assert!(out.contains("version:        0.2.1"));
        // Other lines unchanged.
        assert!(out.contains("name:           synapse"));
        assert!(out.contains("license:        MIT"));
    }

    #[test]
    fn bump_cabal_minor() {
        let cabal = "name: x\nversion: 1.4.7\n";
        let (_, new, _) = bump_cabal_file(cabal, "minor").unwrap();
        assert_eq!(new, "1.5.0");
    }

    #[test]
    fn bump_cabal_exact_target() {
        let cabal = "name: x\nversion: 0.1.0\n";
        let (_, new, out) = bump_cabal_file(cabal, "3.0.0").unwrap();
        assert_eq!(new, "3.0.0");
        assert!(out.contains("version: 3.0.0"));
    }

    #[test]
    fn bump_cabal_missing_version() {
        let cabal = "name: x\nlicense: MIT\n";
        assert!(bump_cabal_file(cabal, "patch").is_err());
    }

    #[test]
    fn bump_cabal_preserves_trailing_newline() {
        let cabal = "name: x\nversion: 1.0.0\n";
        let (_, _, out) = bump_cabal_file(cabal, "patch").unwrap();
        assert!(out.ends_with('\n'));
    }

    // ── Publish command tests ───────────────────────────────────────

    #[test]
    fn hackage_publish_command() {
        let cmd = publish_command("hackage", "tok123").unwrap();
        assert!(cmd.contains("cabal sdist"));
        assert!(cmd.contains("cabal upload --publish"));
        assert!(cmd.contains("--token="));
        assert!(cmd.contains("tok123"));
    }

    #[test]
    fn crates_io_publish_command() {
        let cmd = publish_command("crates.io", "secret").unwrap();
        assert!(cmd.contains("CARGO_REGISTRY_TOKEN="));
        assert!(cmd.contains("cargo publish"));
    }

    #[test]
    fn no_allow_dirty_in_any_publish_command() {
        // HF-PUBLISH-SAFETY AC-3: --allow-dirty appears in NO publish
        // command builder. Dirty trees are pre-flight hard errors.
        for ch in ["crates.io", "npm", "pypi", "hackage"] {
            let cmd = publish_command(ch, "tok").unwrap();
            assert!(
                !cmd.contains("--allow-dirty"),
                "{ch} publish command still carries --allow-dirty: {cmd}"
            );
        }
    }

    #[test]
    fn unknown_channel_returns_none() {
        assert!(publish_command("bogus", "x").is_none());
    }

    #[test]
    fn secret_key_hackage() {
        assert_eq!(secret_key_for_channel("hackage"), Some("hackage/token"));
    }

    #[test]
    fn secret_key_unknown() {
        assert_eq!(secret_key_for_channel("bogus"), None);
    }

    #[test]
    fn shell_escape_single_quotes() {
        let escaped = shell_escape("it's a test");
        assert_eq!(escaped, "'it'\\''s a test'");
    }
}
