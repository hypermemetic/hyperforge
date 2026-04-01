# AUTH-2: Credential Registry

blocked_by: []
unlocks: [AUTH-3, AUTH-4, AUTH-5]

## Scope

Build a static registry that catalogs every credential hyperforge knows about — what it's called, where to get it, what scopes it needs, and how to validate it. This is the foundation the other AUTH tickets build on.

## Implementation

### New file: `src/auth/credentials.rs`

```rust
/// A known credential that hyperforge can work with
pub struct CredentialSpec {
    /// Key path in secrets store (e.g. "github/{org}/token")
    pub key_pattern: &'static str,
    /// Human-readable name
    pub display_name: &'static str,
    /// What kind of credential
    pub kind: CredentialKind,
    /// URL where the user creates this credential
    pub setup_url: &'static str,
    /// Setup instructions
    pub instructions: &'static str,
    /// How to validate
    pub validation: ValidationMethod,
    /// Which distribution channels need this credential
    pub required_by: &'static [&'static str],
}

pub enum CredentialKind {
    BearerToken,
    ClassicPat { required_scopes: &'static [&'static str] },
    UsernamePassword,
    ApiKey,
}

pub enum ValidationMethod {
    /// GET endpoint, expect 200
    HttpGet { url_pattern: &'static str, auth_scheme: &'static str },
    /// GET /user on GitHub, check X-OAuth-Scopes header
    GitHubScopes { required: &'static [&'static str] },
    /// Just check the secret exists
    ExistsOnly,
}
```

### The registry (static data)

```rust
pub const CREDENTIALS: &[CredentialSpec] = &[
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
        instructions: "Create a CLASSIC token (not fine-grained) with read:packages and write:packages scopes",
        // ...
    },
    // codeberg, gitlab, crates-io, hackage, npm, cachix...
];
```

### Helper functions

```rust
/// Get all credentials needed for a set of distribution channels
pub fn credentials_for_channels(channels: &[DistChannel], org: &str) -> Vec<ResolvedCredential>

/// Get all credentials needed for a forge
pub fn credentials_for_forge(forge: &Forge, org: &str) -> Vec<ResolvedCredential>

/// Resolve {org} in key_pattern to actual key path
pub fn resolve_key_path(pattern: &str, org: &str) -> String
```

## Acceptance Criteria

- [ ] CredentialSpec covers all known credential types (GitHub, Codeberg, GitLab, crates.io, Hackage, npm, Cachix)
- [ ] `credentials_for_channels` correctly maps DistChannel → required credentials
- [ ] `resolve_key_path` substitutes {org} in patterns
- [ ] Unit tests for channel → credential mapping
