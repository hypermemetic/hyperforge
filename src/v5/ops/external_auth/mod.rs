//! `ops::external_auth` — typed wrapper over forge-CLI auth subprocesses
//! (V5PARITY-27).
//!
//! Same shape as `ops::git`: this module is the only place that spawns
//! `gh` (and, when the providers land, `glab` / `berg`) processes. The
//! V5LIFECYCLE-11 `command-gh` DRY grep enforces it.
//!
//! Tokens never appear in event payloads, `Debug` impls, or logs. The
//! sole token-returning function is `read_token`; callers consume the
//! token immediately and store it via `secrets.set`.

use std::process::Command;

use thiserror::Error;

mod gh;

/// Closed set of forge auth providers v5 understands. v1 implements
/// only `Github`; the others return `CliNotFound` until each provider's
/// CLI module lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAuthProvider {
    Github,
    Codeberg,
    Gitlab,
}

impl ExternalAuthProvider {
    #[must_use]
    pub const fn cli(&self) -> &'static str {
        match self {
            Self::Github => "gh",
            Self::Codeberg => "berg",
            Self::Gitlab => "glab",
        }
    }
}

impl std::fmt::Display for ExternalAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Github => "github",
            Self::Codeberg => "codeberg",
            Self::Gitlab => "gitlab",
        })
    }
}

/// Closed error set for external-auth ops. Maps cleanly to event
/// `code` strings for V5PARITY-21's `bootstrap_failed.stage`.
#[derive(Debug, Clone, Error)]
pub enum ExternalAuthError {
    #[error("{provider} cli not found")]
    CliNotFound { provider: ExternalAuthProvider },
    #[error("{provider} cli not logged in")]
    NotLoggedIn { provider: ExternalAuthProvider },
    #[error("network: {0}")]
    Network(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("io: {0}")]
    Io(String),
    #[error("provider {0:?} not implemented in v1")]
    Unimplemented(ExternalAuthProvider),
}

impl ExternalAuthError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CliNotFound { .. } => "cli_not_found",
            Self::NotLoggedIn { .. } => "not_logged_in",
            Self::Network(_) => "network",
            Self::InvalidResponse(_) => "invalid_response",
            Self::Io(_) => "io",
            Self::Unimplemented(_) => "unimplemented",
        }
    }
}

/// Snapshot of an external CLI's auth state. Token values are never
/// included; only metadata callers need to make registration decisions.
///
/// Manual `Debug` so future field additions can't accidentally include
/// a token field that appears in logs.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct ExternalAuthStatus {
    pub provider: ExternalAuthProvider,
    pub logged_in: bool,
    pub username: Option<String>,
    pub host: Option<String>,
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for ExternalAuthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalAuthStatus")
            .field("provider", &self.provider)
            .field("logged_in", &self.logged_in)
            .field("username", &self.username)
            .field("host", &self.host)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Returns `None` when the CLI isn't installed (not an error — absence
/// of auth is a fact, not a failure). `Some(_)` always for installed
/// CLIs; `logged_in: false` is the not-logged-in case.
#[must_use]
pub fn detect_status(provider: ExternalAuthProvider) -> Option<ExternalAuthStatus> {
    if !cli_installed(provider) {
        return None;
    }
    match provider {
        ExternalAuthProvider::Github => gh::detect_status().ok(),
        _ => None,
    }
}

/// Returns the token string or an error. Callers must consume it
/// immediately and never log it.
pub fn read_token(provider: ExternalAuthProvider) -> Result<String, ExternalAuthError> {
    if !cli_installed(provider) {
        return Err(ExternalAuthError::CliNotFound { provider });
    }
    match provider {
        ExternalAuthProvider::Github => gh::read_token(),
        p => Err(ExternalAuthError::Unimplemented(p)),
    }
}

/// Returns org names visible via the CLI's auth.
pub fn list_accessible_orgs(provider: ExternalAuthProvider) -> Result<Vec<String>, ExternalAuthError> {
    if !cli_installed(provider) {
        return Err(ExternalAuthError::CliNotFound { provider });
    }
    match provider {
        ExternalAuthProvider::Github => gh::list_accessible_orgs(),
        p => Err(ExternalAuthError::Unimplemented(p)),
    }
}

fn cli_installed(provider: ExternalAuthProvider) -> bool {
    Command::new(provider.cli())
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_providers_dont_panic() {
        // gitlab/codeberg providers return Unimplemented or CliNotFound.
        let _ = read_token(ExternalAuthProvider::Gitlab);
        let _ = list_accessible_orgs(ExternalAuthProvider::Codeberg);
    }

    #[test]
    fn debug_does_not_leak_token_field() {
        // ExternalAuthStatus deliberately has no `token` field; Debug
        // is hand-written. Add a guard test so a future field addition
        // gets caught.
        let s = ExternalAuthStatus {
            provider: ExternalAuthProvider::Github,
            logged_in: true,
            username: Some("u".into()),
            host: Some("github.com".into()),
            scopes: vec!["repo".into()],
        };
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("token"));
        assert!(!dbg.contains("Token"));
    }
}
