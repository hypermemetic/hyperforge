//! `ops::git` — subprocess wrappers over the user's `git` CLI (V5PARITY-3).
//!
//! Per ticket R1: shell out to `git` rather than using libgit2, so the
//! user's SSH agent / credential helper / hooks / config all flow
//! through transparently. D13: this is the ONLY module in src/v5/ that
//! spawns `git` processes; hubs route through these helpers.

use std::path::Path;
use std::process::Command;

use thiserror::Error;

/// Git operation error. Narrow variants for the cases callers branch
/// on; `Other` for everything else.
#[derive(Debug, Error, Clone)]
pub enum GitError {
    #[error("git not found on PATH")]
    GitNotFound,
    #[error("target path is not a git working tree: {0}")]
    NotAGitRepo(String),
    #[error("working tree is dirty: {0}")]
    DirtyTree(String),
    #[error("destination already exists: {0}")]
    DestExists(String),
    #[error("non-fast-forward — branch has diverged")]
    NonFastForward,
    #[error("git command failed ({code}): {stderr}")]
    CommandFailed { code: i32, stderr: String },
    #[error("io error: {0}")]
    Io(String),
}

impl GitError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::GitNotFound => "git_not_found",
            Self::NotAGitRepo(_) => "not_a_git_repo",
            Self::DirtyTree(_) => "dirty_tree",
            Self::DestExists(_) => "dest_exists",
            Self::NonFastForward => "non_ff",
            Self::CommandFailed { .. } => "git_failed",
            Self::Io(_) => "io",
        }
    }
}

/// Parsed output of `git status --porcelain=v2 --branch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
}

impl StatusSnapshot {
    #[must_use]
    pub const fn dirty(&self) -> bool {
        self.staged > 0 || self.unstaged > 0 || self.untracked > 0
    }
}

/// `git clone <url> <dest>` with optional transport flip. Refuses if
/// `dest` already exists.
pub fn clone_repo(url: &str, dest: &Path) -> Result<(), GitError> {
    clone_repo_with_env(url, dest, &[])
}

/// `git clone <url> <dest>` with extra environment variables. Callers
/// that need to forward a per-repo `GIT_SSH_COMMAND` (V5PARITY-5) do so
/// via this variant; a regular clone uses `clone_repo`.
pub fn clone_repo_with_env(
    url: &str,
    dest: &Path,
    env: &[(&str, &str)],
) -> Result<(), GitError> {
    if dest.exists() {
        return Err(GitError::DestExists(dest.display().to_string()));
    }
    run_git_with_env(
        None,
        &["clone", url, dest.to_str().unwrap_or("")],
        env,
    )
}

/// `git -C <dir> fetch [<remote>]`. `None` fetches all remotes.
pub fn fetch(dir: &Path, remote: Option<&str>) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    let mut args: Vec<&str> = vec!["-C", dir.to_str().unwrap_or(""), "fetch"];
    if let Some(r) = remote {
        args.push(r);
    } else {
        args.push("--all");
    }
    run_git(None, &args)
}

/// `git -C <dir> pull --ff-only <remote> <branch>`. Errors with
/// `DirtyTree` if the tree has uncommitted changes; `NonFastForward`
/// if the branch has diverged.
pub fn pull_ff(dir: &Path, remote: &str, branch: &str) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    if is_dirty(dir)? {
        return Err(GitError::DirtyTree(dir.display().to_string()));
    }
    match run_git(
        None,
        &["-C", dir.to_str().unwrap_or(""), "pull", "--ff-only", remote, branch],
    ) {
        Ok(()) => Ok(()),
        Err(GitError::CommandFailed { stderr, .. }) if stderr.contains("Not possible to fast-forward") => {
            Err(GitError::NonFastForward)
        }
        Err(e) => Err(e),
    }
}

/// `git -C <dir> push <remote> [<branch>]`.
pub fn push_refs(dir: &Path, remote: &str, branch: Option<&str>) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    let mut args: Vec<&str> = vec!["-C", dir.to_str().unwrap_or(""), "push", remote];
    if let Some(b) = branch {
        args.push(b);
    }
    run_git(None, &args)
}

/// `git -C <dir> status --porcelain=v2 --branch` parsed into typed form.
pub fn status(dir: &Path) -> Result<StatusSnapshot, GitError> {
    ensure_git_repo(dir)?;
    let out = run_git_capture(
        None,
        &["-C", dir.to_str().unwrap_or(""), "status", "--porcelain=v2", "--branch"],
    )?;
    Ok(parse_status(&out))
}

/// Shortcut: `status(dir).dirty`.
pub fn is_dirty(dir: &Path) -> Result<bool, GitError> {
    status(dir).map(|s| s.dirty())
}

/// `git -C <dir> remote set-url <name> <url>`. Idempotent at the git
/// level: setting to the same URL is a no-op write.
pub fn set_remote_url(dir: &Path, name: &str, url: &str) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    run_git(
        None,
        &["-C", dir.to_str().unwrap_or(""), "remote", "set-url", name, url],
    )
}

// ---------------------------------------------------------------------
// V5PARITY-10: commit + tag helpers. Routing build-release through
// ops::git keeps D13's "one subprocess entry point" invariant; see
// V5LIFECYCLE-11's `command-git` DRY grep.
// ---------------------------------------------------------------------

/// `git -C <dir> add <path>`. Accepts one or more paths.
pub fn add(dir: &Path, paths: &[&str]) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    let mut args: Vec<&str> = vec!["-C", dir.to_str().unwrap_or(""), "add"];
    args.extend_from_slice(paths);
    run_git(None, &args)
}

/// `git -C <dir> commit -m <message>`. Fails with `CommandFailed` if
/// there's nothing staged; callers that want "commit if there's
/// anything to commit" should check `status()` first.
pub fn commit(dir: &Path, message: &str) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    run_git(
        None,
        &["-C", dir.to_str().unwrap_or(""), "commit", "-m", message],
    )
}

/// `git -C <dir> tag <name>`. Lightweight tag.
pub fn tag(dir: &Path, name: &str) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    run_git(
        None,
        &["-C", dir.to_str().unwrap_or(""), "tag", name],
    )
}

/// `git -C <dir> push <remote> <refspec>`. Used by V5PARITY-10's
/// release flow to push both branch and tag. `push_refs` pushes all
/// refs; `push_ref` is explicit.
pub fn push_ref(dir: &Path, remote: &str, refspec: &str) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    run_git(
        None,
        &["-C", dir.to_str().unwrap_or(""), "push", remote, refspec],
    )
}

/// `git -C <dir> show <rev>:<path>`. Returns the file contents as
/// seen at `rev`. Used by V5PARITY-9's `build.package_diff`.
pub fn show(dir: &Path, rev: &str, path: &str) -> Result<String, GitError> {
    ensure_git_repo(dir)?;
    run_git_capture(
        None,
        &["-C", dir.to_str().unwrap_or(""), "show", &format!("{rev}:{path}")],
    )
}

// ---------------------------------------------------------------------
// V5PARITY-5: per-repo SSH command.
//
// Writes `core.sshCommand = ssh -i <key> -o IdentitiesOnly=yes` into
// the repo's `.git/config` via `git config`. Never touches
// `~/.ssh/config`. `IdentitiesOnly=yes` prevents ssh-agent from
// silently preferring a different key that happens to be loaded.
// ---------------------------------------------------------------------

/// Format the ssh command we write into `.git/config`. Exposed for
/// tests and callers that need to pass the same string as
/// `GIT_SSH_COMMAND` during a clone.
#[must_use]
pub fn format_ssh_command(key_path: &Path) -> String {
    format!("ssh -i {} -o IdentitiesOnly=yes", key_path.display())
}

/// Set the per-repo `core.sshCommand`. Idempotent.
pub fn set_ssh_command(dir: &Path, key_path: &Path) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    let cmd = format_ssh_command(key_path);
    run_git(
        None,
        &["-C", dir.to_str().unwrap_or(""), "config", "core.sshCommand", &cmd],
    )
}

/// Clear a previously-set `core.sshCommand`. A missing entry is not
/// treated as an error.
pub fn clear_ssh_command(dir: &Path) -> Result<(), GitError> {
    ensure_git_repo(dir)?;
    match run_git(
        None,
        &["-C", dir.to_str().unwrap_or(""), "config", "--unset", "core.sshCommand"],
    ) {
        Ok(()) => Ok(()),
        // Exit 5 from `git config --unset` means "key not set" — not
        // an error for our idempotent clear.
        Err(GitError::CommandFailed { code: 5, .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Read the current `core.sshCommand`. Returns `None` if unset.
pub fn get_ssh_command(dir: &Path) -> Result<Option<String>, GitError> {
    ensure_git_repo(dir)?;
    match run_git_capture(
        None,
        &["-C", dir.to_str().unwrap_or(""), "config", "--get", "core.sshCommand"],
    ) {
        Ok(s) => Ok(Some(s.trim().to_string())),
        Err(GitError::CommandFailed { code: 1, .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------
// Internals.
// ---------------------------------------------------------------------

fn ensure_git_repo(dir: &Path) -> Result<(), GitError> {
    let git_dir = dir.join(".git");
    if !git_dir.exists() {
        return Err(GitError::NotAGitRepo(dir.display().to_string()));
    }
    Ok(())
}

fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<(), GitError> {
    run_git_with_env(cwd, args, &[])
}

fn run_git_with_env(cwd: Option<&Path>, args: &[&str], env: &[(&str, &str)]) -> Result<(), GitError> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    match cmd.output() {
        Ok(out) => {
            if out.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                let code = out.status.code().unwrap_or(-1);
                Err(GitError::CommandFailed { code, stderr })
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(GitError::GitNotFound),
        Err(e) => Err(GitError::Io(e.to_string())),
    }
}

fn run_git_capture(cwd: Option<&Path>, args: &[&str]) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    match cmd.output() {
        Ok(out) => {
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                let code = out.status.code().unwrap_or(-1);
                Err(GitError::CommandFailed { code, stderr })
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(GitError::GitNotFound),
        Err(e) => Err(GitError::Io(e.to_string())),
    }
}

/// Parse `git status --porcelain=v2 --branch` output into a snapshot.
/// Porcelain v2 format:
///   # branch.oid <oid>
///   # branch.head <branch>
///   # branch.upstream <upstream>
///   # branch.ab +N -M
///   1 <status> ... (staged/unstaged)
///   ? <path>      (untracked)
fn parse_status(raw: &str) -> StatusSnapshot {
    let mut branch = None;
    let mut upstream = None;
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut staged = 0u32;
    let mut unstaged = 0u32;
    let mut untracked = 0u32;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            branch = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            upstream = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut it = rest.split_whitespace();
            if let Some(a) = it.next() {
                ahead = a.trim_start_matches('+').parse().unwrap_or(0);
            }
            if let Some(b) = it.next() {
                behind = b.trim_start_matches('-').parse().unwrap_or(0);
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // Ordinary / renamed changed entries. The 2nd field is the
            // XY status codes: X = staged, Y = unstaged.
            let fields: Vec<&str> = line.splitn(3, ' ').collect();
            if fields.len() >= 2 {
                let xy = fields[1].as_bytes();
                if xy.first().is_some_and(|&c| c != b'.') {
                    staged += 1;
                }
                if xy.get(1).is_some_and(|&c| c != b'.') {
                    unstaged += 1;
                }
            }
        } else if line.starts_with("? ") {
            untracked += 1;
        }
    }
    StatusSnapshot {
        branch,
        upstream,
        ahead,
        behind,
        staged,
        unstaged,
        untracked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_clean() {
        let raw = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n";
        let s = parse_status(raw);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.ahead, 0);
        assert_eq!(s.behind, 0);
        assert!(!s.dirty());
    }

    #[test]
    fn parse_status_dirty() {
        let raw = "# branch.head main\n# branch.ab +2 -1\n1 M. N... 100644 100644 100644 aaa bbb file1\n? unknown.txt\n";
        let s = parse_status(raw);
        assert_eq!(s.ahead, 2);
        assert_eq!(s.behind, 1);
        assert_eq!(s.staged, 1);
        assert_eq!(s.untracked, 1);
        assert!(s.dirty());
    }
}
