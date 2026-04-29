//! `gh` CLI subprocess backend (V5PARITY-27).
//!
//! `gh auth status` output format we parse:
//!
//! ```text
//! github.com
//!   ✓ Logged in to github.com account hypermemetic (keyring)
//!   - Active account: true
//!   - Token: gho_*****
//!   - Token scopes: 'repo', 'read:org'
//! ```
//!
//! Output goes to stderr, not stdout; `gh auth status` returns exit 0
//! when logged in and exit 1 otherwise.

use std::process::Command;

use super::{ExternalAuthError, ExternalAuthProvider, ExternalAuthStatus};

const PROVIDER: ExternalAuthProvider = ExternalAuthProvider::Github;

pub(super) fn detect_status() -> Result<ExternalAuthStatus, ExternalAuthError> {
    let out = Command::new("gh")
        .args(["auth", "status"])
        .output()
        .map_err(|e| ExternalAuthError::Io(e.to_string()))?;
    let combined = String::from_utf8_lossy(&out.stderr).into_owned()
        + &String::from_utf8_lossy(&out.stdout);
    let logged_in = out.status.success();
    if !logged_in {
        return Ok(ExternalAuthStatus {
            provider: PROVIDER,
            logged_in: false,
            username: None,
            host: None,
            scopes: Vec::new(),
        });
    }
    Ok(parse_status(&combined))
}

pub(super) fn read_token() -> Result<String, ExternalAuthError> {
    let out = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .map_err(|e| ExternalAuthError::Io(e.to_string()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not logged") || stderr.contains("no auth") {
            return Err(ExternalAuthError::NotLoggedIn { provider: PROVIDER });
        }
        return Err(ExternalAuthError::InvalidResponse(stderr.into_owned()));
    }
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if token.is_empty() {
        return Err(ExternalAuthError::NotLoggedIn { provider: PROVIDER });
    }
    Ok(token)
}

pub(super) fn list_accessible_orgs() -> Result<Vec<String>, ExternalAuthError> {
    let out = Command::new("gh")
        .args(["api", "/user/orgs", "--jq", ".[].login"])
        .output()
        .map_err(|e| ExternalAuthError::Io(e.to_string()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not logged") || stderr.contains("authentication required") {
            return Err(ExternalAuthError::NotLoggedIn { provider: PROVIDER });
        }
        return Err(ExternalAuthError::Network(stderr.into_owned()));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

fn parse_status(text: &str) -> ExternalAuthStatus {
    let mut username = None;
    let mut host = None;
    let mut scopes: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        // "Logged in to <host> account <user>" — the host appears
        // before "account" and the user after.
        if line.contains("Logged in to") {
            if let Some(rest) = line.split("Logged in to").nth(1) {
                let mut it = rest.split_whitespace();
                host = it.next().map(String::from);
                if let Some(account_pos) = rest.find("account ") {
                    let after = &rest[account_pos + "account ".len()..];
                    username = after.split_whitespace().next().map(String::from);
                }
            }
        }
        if let Some(rest) = line.strip_prefix("- Token scopes:")
            .or_else(|| line.strip_prefix("Token scopes:"))
        {
            scopes = rest
                .split(',')
                .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    ExternalAuthStatus {
        provider: PROVIDER,
        logged_in: true,
        username,
        host,
        scopes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_logged_in() {
        let raw = "github.com
  ✓ Logged in to github.com account hypermemetic (keyring)
  - Active account: true
  - Git operations protocol: https
  - Token: gho_*****
  - Token scopes: 'gist', 'read:org', 'repo'
";
        let s = parse_status(raw);
        assert_eq!(s.username.as_deref(), Some("hypermemetic"));
        assert_eq!(s.host.as_deref(), Some("github.com"));
        assert_eq!(s.scopes, vec!["gist", "read:org", "repo"]);
    }

    #[test]
    fn parse_status_no_scopes_line() {
        let raw = "github.com\n  ✓ Logged in to github.com account demo (keyring)\n";
        let s = parse_status(raw);
        assert_eq!(s.username.as_deref(), Some("demo"));
        assert!(s.scopes.is_empty());
    }
}
