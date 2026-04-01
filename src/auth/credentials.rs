//! Static credential registry for all known secrets hyperforge manages.
//!
//! Each credential has a spec describing its key pattern, kind, validation
//! method, and which distribution channels / forges require it.

use crate::types::config::DistChannel;
use crate::types::Forge;

/// Kind of credential
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialKind {
    /// Simple bearer/PAT token
    BearerToken,
    /// Classic PAT requiring specific OAuth scopes
    ClassicPat {
        required_scopes: &'static [&'static str],
    },
    /// Username + password pair (two secrets — each half is its own spec)
    UsernamePassword,
    /// Simple API key
    ApiKey,
}

/// How to validate a credential
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationMethod {
    /// GET an endpoint with auth, expect 200
    HttpGet {
        url_pattern: &'static str,
        auth_scheme: &'static str,
    },
    /// GET GitHub /user, parse X-OAuth-Scopes header
    GitHubScopes {
        required: &'static [&'static str],
    },
    /// Just check the secret exists in the store
    ExistsOnly,
}

/// A known credential specification
#[derive(Debug, Clone)]
pub struct CredentialSpec {
    /// Key path pattern in secrets store (e.g. "github/{org}/token")
    pub key_pattern: &'static str,
    /// Human-readable name
    pub display_name: &'static str,
    /// What kind of credential
    pub kind: CredentialKind,
    /// URL where user creates this credential
    pub setup_url: &'static str,
    /// Setup instructions
    pub instructions: &'static str,
    /// How to validate
    pub validation: ValidationMethod,
    /// Which distribution channels / roles need this credential
    pub required_by: &'static [&'static str],
}

/// A resolved credential with {org} substituted
#[derive(Debug, Clone)]
pub struct ResolvedCredential {
    pub spec: &'static CredentialSpec,
    pub key_path: String,
    pub org: String,
}

// ---------------------------------------------------------------------------
// Static registry
// ---------------------------------------------------------------------------

pub const CREDENTIALS: &[CredentialSpec] = &[
    // ── GitHub ────────────────────────────────────────────────────────
    CredentialSpec {
        key_pattern: "github/{org}/token",
        display_name: "GitHub Personal Access Token",
        kind: CredentialKind::BearerToken,
        setup_url: "https://github.com/settings/tokens/new",
        instructions: "Create a fine-grained PAT with 'Contents: Read and write' permission for the org",
        validation: ValidationMethod::HttpGet {
            url_pattern: "https://api.github.com/user",
            auth_scheme: "Bearer",
        },
        required_by: &["forge-release", "sync"],
    },
    CredentialSpec {
        key_pattern: "github/{org}/packages_token",
        display_name: "GitHub Classic PAT (packages)",
        kind: CredentialKind::ClassicPat {
            required_scopes: &["read:packages", "write:packages"],
        },
        setup_url: "https://github.com/settings/tokens/new?scopes=read:packages,write:packages",
        instructions: "Create a CLASSIC token (not fine-grained) with read:packages and write:packages scopes",
        validation: ValidationMethod::GitHubScopes {
            required: &["read:packages", "write:packages"],
        },
        required_by: &["ghcr"],
    },
    // ── Codeberg ─────────────────────────────────────────────────────
    CredentialSpec {
        key_pattern: "codeberg/{org}/token",
        display_name: "Codeberg Personal Access Token",
        kind: CredentialKind::BearerToken,
        setup_url: "https://codeberg.org/user/settings/applications",
        instructions: "Create a token with repository read/write permissions",
        validation: ValidationMethod::HttpGet {
            url_pattern: "https://codeberg.org/api/v1/user",
            auth_scheme: "Bearer",
        },
        required_by: &["forge-release", "sync"],
    },
    // ── GitLab ───────────────────────────────────────────────────────
    CredentialSpec {
        key_pattern: "gitlab/{org}/token",
        display_name: "GitLab Personal Access Token",
        kind: CredentialKind::BearerToken,
        setup_url: "https://gitlab.com/-/user_settings/personal_access_tokens",
        instructions: "Create a token with api scope",
        validation: ValidationMethod::HttpGet {
            url_pattern: "https://gitlab.com/api/v4/user",
            auth_scheme: "Bearer",
        },
        required_by: &["forge-release", "sync"],
    },
    // ── crates.io ────────────────────────────────────────────────────
    CredentialSpec {
        key_pattern: "crates-io/token",
        display_name: "crates.io API Token",
        kind: CredentialKind::ApiKey,
        setup_url: "https://crates.io/settings/tokens",
        instructions: "Create an API token with publish-update scope",
        validation: ValidationMethod::HttpGet {
            url_pattern: "https://crates.io/api/v1/me",
            auth_scheme: "Bearer",
        },
        required_by: &["crates-io"],
    },
    // ── Hackage ──────────────────────────────────────────────────────
    CredentialSpec {
        key_pattern: "hackage/username",
        display_name: "Hackage Username",
        kind: CredentialKind::UsernamePassword,
        setup_url: "https://hackage.haskell.org/users/register",
        instructions: "Your Hackage account username",
        validation: ValidationMethod::ExistsOnly,
        required_by: &["hackage"],
    },
    CredentialSpec {
        key_pattern: "hackage/password",
        display_name: "Hackage Password",
        kind: CredentialKind::UsernamePassword,
        setup_url: "https://hackage.haskell.org/users/register",
        instructions: "Your Hackage account password",
        validation: ValidationMethod::ExistsOnly,
        required_by: &["hackage"],
    },
    // ── npm (future) ─────────────────────────────────────────────────
    CredentialSpec {
        key_pattern: "npm/token",
        display_name: "npm Access Token",
        kind: CredentialKind::ApiKey,
        setup_url: "https://www.npmjs.com/settings/tokens",
        instructions: "Create an automation token with publish permission",
        validation: ValidationMethod::ExistsOnly,
        required_by: &["npm"],
    },
    // ── Cachix / Nix (future) ────────────────────────────────────────
    CredentialSpec {
        key_pattern: "cachix/{org}/token",
        display_name: "Cachix Auth Token",
        kind: CredentialKind::ApiKey,
        setup_url: "https://app.cachix.org",
        instructions: "Create an auth token for your Cachix cache",
        validation: ValidationMethod::ExistsOnly,
        required_by: &["nix"],
    },
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve `{org}` placeholders in a key pattern.
pub fn resolve_key_path(pattern: &str, org: &str) -> String {
    pattern.replace("{org}", org)
}

/// Get all credentials needed for a set of distribution channels + org.
pub fn credentials_for_channels(
    channels: &[DistChannel],
    org: &str,
) -> Vec<ResolvedCredential> {
    let channel_tags: Vec<String> = channels.iter().map(|c| c.to_string()).collect();

    let mut out = Vec::new();
    for spec in CREDENTIALS {
        let needed = spec
            .required_by
            .iter()
            .any(|tag| channel_tags.iter().any(|ct| ct == *tag));
        if needed {
            out.push(ResolvedCredential {
                spec,
                key_path: resolve_key_path(spec.key_pattern, org),
                org: org.to_string(),
            });
        }
    }
    out
}

/// Get all credentials needed for a forge + org.
///
/// Returns the primary forge token (not packages/GHCR tokens — those are
/// channel-driven via `credentials_for_channels`).
pub fn credentials_for_forge(forge: &Forge, org: &str) -> Vec<ResolvedCredential> {
    let prefix = match forge {
        Forge::GitHub => "github/",
        Forge::Codeberg => "codeberg/",
        Forge::GitLab => "gitlab/",
    };

    // The primary token pattern for each forge is `<forge>/{org}/token`
    let primary_pattern = format!("{prefix}{{org}}/token");

    CREDENTIALS
        .iter()
        .filter(|spec| spec.key_pattern == primary_pattern)
        .map(|spec| ResolvedCredential {
            spec,
            key_path: resolve_key_path(spec.key_pattern, org),
            org: org.to_string(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_key_path_substitutes_org() {
        assert_eq!(
            resolve_key_path("github/{org}/token", "myorg"),
            "github/myorg/token"
        );
        assert_eq!(
            resolve_key_path("crates-io/token", "myorg"),
            "crates-io/token"
        );
    }

    #[test]
    fn credentials_for_forge_release_returns_github_token() {
        let creds = credentials_for_channels(&[DistChannel::ForgeRelease], "acme");
        let keys: Vec<&str> = creds.iter().map(|c| c.key_path.as_str()).collect();
        assert!(keys.contains(&"github/acme/token"), "expected github token, got {keys:?}");
        assert!(keys.contains(&"codeberg/acme/token"));
        assert!(keys.contains(&"gitlab/acme/token"));
    }

    #[test]
    fn credentials_for_ghcr_returns_packages_token() {
        let creds = credentials_for_channels(&[DistChannel::Ghcr], "acme");
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].key_path, "github/acme/packages_token");
    }

    #[test]
    fn credentials_for_crates_io() {
        let creds = credentials_for_channels(&[DistChannel::CratesIo], "acme");
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].key_path, "crates-io/token");
    }

    #[test]
    fn credentials_for_hackage_returns_both() {
        let creds = credentials_for_channels(&[DistChannel::Hackage], "acme");
        let keys: Vec<&str> = creds.iter().map(|c| c.key_path.as_str()).collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"hackage/username"));
        assert!(keys.contains(&"hackage/password"));
    }

    #[test]
    fn credentials_for_forge_github() {
        let creds = credentials_for_forge(&Forge::GitHub, "myorg");
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].key_path, "github/myorg/token");
    }

    #[test]
    fn credentials_for_forge_codeberg() {
        let creds = credentials_for_forge(&Forge::Codeberg, "myorg");
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].key_path, "codeberg/myorg/token");
    }

    #[test]
    fn credentials_for_forge_gitlab() {
        let creds = credentials_for_forge(&Forge::GitLab, "myorg");
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].key_path, "gitlab/myorg/token");
    }

    #[test]
    fn no_credentials_for_brew_channel() {
        // Brew doesn't require any secret credentials
        let creds = credentials_for_channels(&[DistChannel::Brew], "acme");
        assert!(creds.is_empty());
    }

    #[test]
    fn multiple_channels_dedup() {
        // forge-release + crates-io should return forge tokens + crates token
        let creds = credentials_for_channels(
            &[DistChannel::ForgeRelease, DistChannel::CratesIo],
            "acme",
        );
        let keys: Vec<&str> = creds.iter().map(|c| c.key_path.as_str()).collect();
        assert!(keys.contains(&"github/acme/token"));
        assert!(keys.contains(&"crates-io/token"));
    }
}
