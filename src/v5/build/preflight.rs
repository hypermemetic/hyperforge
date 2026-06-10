//! `build::preflight` — publish pre-flight gates (HF-PUBLISH-SAFETY).
//!
//! Spec input is the Z2H-R1 coherence table (trak `2e4f4b53`): every
//! past publish was cut from an unpushed local branch, one crate's
//! version string regressed below the registry, and `--allow-dirty`
//! was hardcoded so uncommitted work could ship silently. The gates
//! here make each of those failure modes a *named, hard* refusal
//! before any bump/tag/publish mutation happens.
//!
//! Three gates, each producing a [`Finding`] with a stable `code`:
//!
//! | code                 | catches                                        |
//! |----------------------|------------------------------------------------|
//! | `dirty_worktree`     | uncommitted changes (names the files)          |
//! | `version_regression` | local version < registry latest (the 0.5.0-vs- |
//! |                      | 0.5.5 plexus-macros trap)                      |
//! | `unpushed_commits`   | HEAD ahead of upstream / no upstream at all    |
//! |                      | (names the branch)                             |
//!
//! Pure decision functions are separated from I/O so the gate logic is
//! unit-testable without fixture repos; [`run_preflight`] is the I/O
//! wrapper that gathers git state via `ops::git` and applies them.
//!
//! Also here:
//! - [`consumer_pins`] — the post-publish consumer-pin report
//!   (HF-PUBLISH-3 territory, minimal form): which workspace members
//!   declare a version requirement on a just-published package, and
//!   whether that requirement admits the new version.
//! - [`canary_message`] — the documented consumer-canary hook point
//!   (stub): scaffold a fresh consumer via plexus-cli and build it
//!   against the about-to-publish versions. Not yet implemented;
//!   the publish path emits this message so the gap is visible in
//!   every run rather than silently absent.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{manifest, registry};

/// Maximum number of dirty paths named inline before eliding.
const MAX_NAMED_PATHS: usize = 10;

/// A single named pre-flight failure.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    /// Stable machine code: `dirty_worktree` | `version_regression`
    /// | `unpushed_commits` | `git_error`.
    pub code: String,
    /// Human-readable detail (names files / branches / versions).
    pub message: String,
}

impl Finding {
    fn new(code: &str, message: String) -> Self {
        Self { code: code.to_string(), message }
    }
}

// ── Pure gate logic ─────────────────────────────────────────────────

/// Gate 1: dirty worktree. `paths` is the porcelain file list
/// (modified, staged, or untracked). Empty ⇒ pass.
#[must_use]
pub fn dirty_worktree_finding(pkg_name: &str, paths: &[String]) -> Option<Finding> {
    if paths.is_empty() {
        return None;
    }
    let named: Vec<&str> = paths
        .iter()
        .take(MAX_NAMED_PATHS)
        .map(String::as_str)
        .collect();
    let elided = paths.len().saturating_sub(MAX_NAMED_PATHS);
    let suffix = if elided > 0 {
        format!(" (+{elided} more)")
    } else {
        String::new()
    };
    Some(Finding::new(
        "dirty_worktree",
        format!(
            "{pkg_name}: working tree has {} uncommitted path(s): {}{suffix}. \
             Commit or stash before publishing (no --allow-dirty).",
            paths.len(),
            named.join(", "),
        ),
    ))
}

/// Gate 2: version monotonicity vs the registry. A local version
/// strictly below the published latest means a publish would ship
/// (possibly newer) code under an *older* number — or trip an
/// auto-bump into shipping feature work as a patch. Hard refusal;
/// fixing the version string is a deliberate human act.
#[must_use]
pub fn version_regression_finding(
    pkg_name: &str,
    local: &str,
    published: Option<&str>,
) -> Option<Finding> {
    let pub_ver = published?;
    if registry::compare_versions(local, pub_ver) == std::cmp::Ordering::Less {
        return Some(Finding::new(
            "version_regression",
            format!(
                "{pkg_name}: local version {local} is BELOW registry latest {pub_ver}. \
                 Set an honest next version (greater than {pub_ver}) before publishing.",
            ),
        ));
    }
    None
}

/// Gate 3: unpushed commits. The R1 systemic finding: every published
/// crate was cut from a branch that never reached a forge. HEAD must
/// have an upstream and be level with it ("behind" is fine — publish
/// doesn't require pulling, only that everything local is shared).
#[must_use]
pub fn unpushed_finding(
    pkg_name: &str,
    branch: Option<&str>,
    upstream: Option<&str>,
    ahead: u32,
) -> Option<Finding> {
    let branch_name = branch.unwrap_or("(detached HEAD)");
    match upstream {
        None => Some(Finding::new(
            "unpushed_commits",
            format!(
                "{pkg_name}: branch '{branch_name}' has no upstream — \
                 nothing on any forge tracks this publish source. Push the branch first.",
            ),
        )),
        Some(up) if ahead > 0 => Some(Finding::new(
            "unpushed_commits",
            format!(
                "{pkg_name}: branch '{branch_name}' is {ahead} commit(s) ahead of {up}. \
                 Push before publishing so the forge matches the registry.",
            ),
        )),
        Some(_) => None,
    }
}

// ── I/O wrapper ─────────────────────────────────────────────────────

/// Run all gates for one package checkout. `force_dirty` skips ONLY
/// the dirty-worktree gate (the escape hatch — caller is responsible
/// for having confirmed it); version regression and unpushed commits
/// have no escape hatch.
///
/// Git errors are surfaced as findings (`git_error`) rather than
/// silently passing: an unreadable repo is not a publishable repo.
#[must_use]
pub fn run_preflight(
    dir: &Path,
    pkg_name: &str,
    local_version: &str,
    published: Option<&str>,
    force_dirty: bool,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    if !force_dirty {
        match crate::v5::ops::git::changed_paths(dir) {
            Ok(paths) => {
                if let Some(f) = dirty_worktree_finding(pkg_name, &paths) {
                    findings.push(f);
                }
            }
            Err(e) => findings.push(Finding::new(
                "git_error",
                format!("{pkg_name}: cannot read worktree state: {e}"),
            )),
        }
    }

    if let Some(f) = version_regression_finding(pkg_name, local_version, published) {
        findings.push(f);
    }

    match crate::v5::ops::git::status(dir) {
        Ok(snap) => {
            if let Some(f) = unpushed_finding(
                pkg_name,
                snap.branch.as_deref(),
                snap.upstream.as_deref(),
                snap.ahead,
            ) {
                findings.push(f);
            }
        }
        Err(e) => findings.push(Finding::new(
            "git_error",
            format!("{pkg_name}: cannot read branch state: {e}"),
        )),
    }

    findings
}

// ── Consumer-pin report ─────────────────────────────────────────────

/// One workspace consumer of a just-published package.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConsumerPin {
    /// The consuming package's name.
    pub consumer: String,
    /// Its declared version requirement on the published package.
    pub requirement: String,
    /// `true` ⇒ the requirement admits the new version (no action);
    /// `false` ⇒ the consumer needs a pin update + its own release;
    /// `None` ⇒ the requirement could not be interpreted (manual check).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admits_new_version: Option<bool>,
}

/// Report which workspace members depend on `published_pkg` and
/// whether their requirement admits `new_version`. Caret semantics
/// (cargo default): compatible iff the leftmost non-zero component
/// matches and the version is >= the requirement base.
#[must_use]
pub fn consumer_pins(
    published_pkg: &str,
    new_version: &str,
    manifests: &[manifest::PackageManifest],
) -> Vec<ConsumerPin> {
    let mut pins = Vec::new();
    for m in manifests {
        if m.name == published_pkg {
            continue;
        }
        for dep in &m.deps {
            if dep.name == published_pkg {
                pins.push(ConsumerPin {
                    consumer: m.name.clone(),
                    requirement: dep.version.clone(),
                    admits_new_version: req_admits(&dep.version, new_version),
                });
            }
        }
    }
    pins
}

/// Minimal caret-requirement check for `MAJOR[.MINOR[.PATCH]]`-shaped
/// requirements (optionally `^`-prefixed — cargo's default semantics).
/// Returns `None` for anything fancier (ranges, wildcards, paths):
/// those are reported for manual review rather than guessed at.
#[must_use]
pub fn req_admits(req: &str, version: &str) -> Option<bool> {
    let req = req.trim();
    let req = req.strip_prefix('^').unwrap_or(req);
    if req.is_empty()
        || req
            .chars()
            .any(|c| !c.is_ascii_digit() && c != '.')
    {
        return None;
    }
    let parse = |s: &str| -> Option<Vec<u64>> {
        s.split('.').map(|p| p.parse::<u64>().ok()).collect()
    };
    let r = parse(req)?;
    let v = parse(version.split(['-', '+']).next().unwrap_or(version))?;
    if r.is_empty() || v.is_empty() {
        return None;
    }
    // Caret compatibility: leftmost non-zero component must match
    // exactly; everything before it must be zero on both sides.
    let idx = r.iter().position(|&x| x != 0).unwrap_or(r.len() - 1);
    for i in 0..=idx {
        if v.get(i).copied().unwrap_or(0) != r[i] {
            return Some(false);
        }
    }
    // And the version must be >= the requirement base.
    for i in 0..r.len().max(v.len()) {
        let rv = r.get(i).copied().unwrap_or(0);
        let vv = v.get(i).copied().unwrap_or(0);
        match vv.cmp(&rv) {
            std::cmp::Ordering::Greater => return Some(true),
            std::cmp::Ordering::Less => return Some(false),
            std::cmp::Ordering::Equal => {}
        }
    }
    Some(true)
}

// ── Consumer-canary hook point (stub) ───────────────────────────────

/// The consumer-canary step: scaffold a minimal consumer via
/// `plexus-cli` (`plexus new <name>`), point its dependency pins at
/// the about-to-publish versions (path overrides), and `cargo build`
/// it. A red canary blocks the publish. **Not yet implemented** —
/// this returns the documented stub message that the publish path
/// emits so every run shows the gap explicitly.
#[must_use]
pub fn canary_message(packages: &[String]) -> String {
    format!(
        "consumer-canary (stub): would scaffold a fresh consumer via \
         `plexus new` (plexus-cli), pin {} at the about-to-publish \
         version(s), and `cargo build` it before any registry upload. \
         Not yet implemented — publish proceeds without canary coverage.",
        if packages.is_empty() {
            "<none>".to_string()
        } else {
            packages.join(", ")
        }
    )
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // ── dirty gate ──────────────────────────────────────────────────

    #[test]
    fn clean_tree_passes() {
        assert!(dirty_worktree_finding("pkg", &[]).is_none());
    }

    #[test]
    fn dirty_tree_names_files() {
        let f = dirty_worktree_finding(
            "plexus-macros",
            &["src/lib.rs".into(), "README.md".into()],
        )
        .unwrap();
        assert_eq!(f.code, "dirty_worktree");
        assert!(f.message.contains("src/lib.rs"));
        assert!(f.message.contains("README.md"));
        assert!(f.message.contains("plexus-macros"));
    }

    #[test]
    fn dirty_tree_elides_beyond_cap() {
        let paths: Vec<String> = (0..15).map(|i| format!("f{i}.rs")).collect();
        let f = dirty_worktree_finding("pkg", &paths).unwrap();
        assert!(f.message.contains("(+5 more)"));
        assert!(f.message.contains("15 uncommitted path(s)"));
    }

    // ── version regression gate (the plexus-macros trap) ───────────

    #[test]
    fn regressed_version_refused() {
        let f = version_regression_finding("plexus-macros", "0.5.0", Some("0.5.5")).unwrap();
        assert_eq!(f.code, "version_regression");
        assert!(f.message.contains("0.5.0"));
        assert!(f.message.contains("0.5.5"));
    }

    #[test]
    fn equal_version_passes_gate() {
        // up_to_date is handled by the skip path, not a regression.
        assert!(version_regression_finding("p", "1.2.3", Some("1.2.3")).is_none());
    }

    #[test]
    fn ahead_version_passes_gate() {
        assert!(version_regression_finding("p", "0.6.0", Some("0.5.5")).is_none());
    }

    #[test]
    fn unpublished_passes_gate() {
        assert!(version_regression_finding("p", "0.1.0", None).is_none());
    }

    // ── unpushed gate ───────────────────────────────────────────────

    #[test]
    fn ahead_of_upstream_refused_naming_branch() {
        let f = unpushed_finding("plexus-core", Some("release/v0.5.3"), Some("github/main"), 17)
            .unwrap();
        assert_eq!(f.code, "unpushed_commits");
        assert!(f.message.contains("release/v0.5.3"));
        assert!(f.message.contains("17"));
    }

    #[test]
    fn no_upstream_refused() {
        let f = unpushed_finding("p", Some("feature/x"), None, 0).unwrap();
        assert_eq!(f.code, "unpushed_commits");
        assert!(f.message.contains("feature/x"));
        assert!(f.message.contains("no upstream"));
    }

    #[test]
    fn level_with_upstream_passes() {
        assert!(unpushed_finding("p", Some("main"), Some("origin/main"), 0).is_none());
    }

    #[test]
    fn behind_only_passes() {
        // Behind is pull territory, not publish-blocking.
        assert!(unpushed_finding("p", Some("main"), Some("origin/main"), 0).is_none());
    }

    // ── fixture-repo integration (the R1 failure-mode matrix) ──────

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Build origin + clone; returns (tempdir-guard, clone path).
    fn fixture_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        std::fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "--bare", "-b", "main", "."]);
        let work = tmp.path().join("work");
        let out = Command::new("git")
            .args(["clone", origin.to_str().unwrap(), work.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(work.join("Cargo.toml"), "[package]\nname=\"fix\"\nversion=\"0.1.0\"\n")
            .unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "init"]);
        git(&work, &["push", "-u", "origin", "main"]);
        (tmp, work)
    }

    #[test]
    fn fixture_clean_pushed_repo_passes_all_gates() {
        let (_guard, work) = fixture_repo();
        let findings = run_preflight(&work, "fix", "0.6.0", Some("0.5.5"), false);
        assert!(findings.is_empty(), "expected clean pass, got {findings:?}");
    }

    #[test]
    fn fixture_macros_state_yields_three_distinct_failures() {
        // Reproduce R1's plexus-macros state: dirty file + unpushed
        // commit + version string below registry. AC-1: three distinct
        // named failures.
        let (_guard, work) = fixture_repo();
        std::fs::write(work.join("extra.rs"), "// wip\n").unwrap();
        git(&work, &["add", "extra.rs"]);
        git(&work, &["commit", "-m", "local only"]);
        std::fs::write(work.join("dirty.rs"), "// uncommitted\n").unwrap();

        let findings = run_preflight(&work, "plexus-macros", "0.5.0", Some("0.5.5"), false);
        let codes: Vec<&str> = findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"dirty_worktree"), "{codes:?}");
        assert!(codes.contains(&"version_regression"), "{codes:?}");
        assert!(codes.contains(&"unpushed_commits"), "{codes:?}");
        assert_eq!(findings.len(), 3);
        // The dirty failure names the file; the unpushed one names the branch.
        assert!(findings.iter().any(|f| f.message.contains("dirty.rs")));
        assert!(findings.iter().any(|f| f.message.contains("main")));
    }

    #[test]
    fn fixture_force_dirty_skips_only_dirty_gate() {
        let (_guard, work) = fixture_repo();
        std::fs::write(work.join("dirty.rs"), "// uncommitted\n").unwrap();
        let findings = run_preflight(&work, "fix", "0.5.0", Some("0.5.5"), true);
        let codes: Vec<&str> = findings.iter().map(|f| f.code.as_str()).collect();
        assert!(!codes.contains(&"dirty_worktree"), "{codes:?}");
        assert!(codes.contains(&"version_regression"), "{codes:?}");
    }

    #[test]
    fn fixture_untracked_file_counts_as_dirty() {
        let (_guard, work) = fixture_repo();
        std::fs::write(work.join("untracked.txt"), "x").unwrap();
        let findings = run_preflight(&work, "fix", "0.2.0", Some("0.1.0"), false);
        assert!(findings.iter().any(|f| f.code == "dirty_worktree"
            && f.message.contains("untracked.txt")));
    }

    // ── consumer pins ───────────────────────────────────────────────

    fn mani(name: &str, deps: &[(&str, &str)]) -> manifest::PackageManifest {
        manifest::PackageManifest {
            kind: "cargo".into(),
            name: name.into(),
            version: "0.0.0".into(),
            deps: deps
                .iter()
                .map(|(n, v)| manifest::Dep {
                    name: (*n).into(),
                    version: (*v).into(),
                    source: "cargo".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn consumer_pin_report_flags_non_admitting_reqs() {
        let manifests = vec![
            mani("plexus-core", &[("plexus-macros", "0.5.0")]),
            mani("plexus-rpc", &[("plexus-macros", "0.6"), ("plexus-core", "0.5")]),
            mani("plexus-macros", &[]),
        ];
        let pins = consumer_pins("plexus-macros", "0.6.0", &manifests);
        assert_eq!(pins.len(), 2);
        let core = pins.iter().find(|p| p.consumer == "plexus-core").unwrap();
        assert_eq!(core.admits_new_version, Some(false)); // ^0.5.0 ∌ 0.6.0
        let rpc = pins.iter().find(|p| p.consumer == "plexus-rpc").unwrap();
        assert_eq!(rpc.admits_new_version, Some(true)); // ^0.6 ∋ 0.6.0
    }

    #[test]
    fn req_admits_caret_semantics() {
        assert_eq!(req_admits("0.5.0", "0.5.5"), Some(true));
        assert_eq!(req_admits("0.5.0", "0.6.0"), Some(false));
        assert_eq!(req_admits("^0.5", "0.5.9"), Some(true));
        assert_eq!(req_admits("1.2", "1.9.0"), Some(true));
        assert_eq!(req_admits("1.2", "2.0.0"), Some(false));
        assert_eq!(req_admits("1.2.3", "1.2.2"), Some(false));
        assert_eq!(req_admits("0.5", "0.5.0"), Some(true));
        // Uninterpretable reqs are surfaced, not guessed.
        assert_eq!(req_admits(">=0.5, <0.7", "0.6.0"), None);
        assert_eq!(req_admits("*", "1.0.0"), None);
    }

    // ── canary stub ─────────────────────────────────────────────────

    #[test]
    fn canary_stub_names_packages_and_admits_being_a_stub() {
        let msg = canary_message(&["plexus-macros".into(), "plexus-core".into()]);
        assert!(msg.contains("plexus-macros, plexus-core"));
        assert!(msg.contains("stub"));
        assert!(msg.contains("plexus new"));
    }
}
