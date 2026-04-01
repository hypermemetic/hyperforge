# AUTH-1: Token Lifecycle Management

## Goal

Unify token management across all registries and forges — discovery, validation, guided setup, scope checking, and expiry detection. Make it impossible to get 3 API calls deep into a sync before discovering a token is missing or misconfigured.

## Context

Today tokens are manually inserted into the secrets sidecar with no validation:
```bash
synapse -P 44105 secrets auth set_secret --secret_key "github/org/token" --value "$(pbpaste)"
```

Pain points from real usage:
- Fine-grained GitHub PATs silently fail on the packages API (need classic PAT)
- Tokens stored with clipboard garbage (wrong content pasted)
- No way to know which tokens are needed before running a command
- SSO-gated org tokens fail with unhelpful 403
- Sidecar restart + stale tokens
- No scope validation (token exists but lacks `write:packages`)
- crates.io and Hackage tokens have completely different key paths than forges

## Credential Model

### Key path convention (extended)

```
# Forge tokens (per-org)
github/{org}/token              — repo CRUD, releases, sync (needs: repo scope)
github/{org}/packages_token     — ghcr.io, packages API (needs: read:packages, write:packages, classic PAT only)
codeberg/{org}/token            — all Codeberg operations
gitlab/{org}/token              — all GitLab operations

# Package registry tokens (global or per-scope)
crates-io/token                 — cargo publish (from https://crates.io/me)
hackage/username                — cabal upload
hackage/password                — cabal upload
npm/token                       — npm publish

# Binary cache tokens
cachix/{cache}/token            — nix binary cache push

# Signing keys
gpg/{org}/key_id                — apt/deb repo signing
```

### Credential type system

```rust
enum CredentialKind {
    /// Bearer/PAT token
    Token,
    /// Username + password pair
    UsernamePassword,
    /// SSH key path
    SshKey,
    /// GPG key ID
    GpgKey,
    /// API key with specific required scopes
    ScopedToken { required_scopes: Vec<String> },
}

struct CredentialSpec {
    /// Key path in secrets store
    key_path: String,
    /// Human-readable name
    display_name: String,
    /// What kind of credential
    kind: CredentialKind,
    /// How to obtain it (URL or instructions)
    setup_url: String,
    /// How to validate it
    validation: CredentialValidation,
}

enum CredentialValidation {
    /// Hit an API endpoint, check for 200
    HttpGet { url: String, auth_header: String },
    /// Hit endpoint and check response header for scopes
    GitHubScopes { required: Vec<String> },
    /// Just check it exists (can't validate without using it)
    ExistsOnly,
}
```

## Dependency DAG

```
AUTH-2 (Credential registry — knows all possible credentials)
  │
  ├──► AUTH-3 (auth check — validate all tokens)
  │
  ├──► AUTH-4 (auth setup — guided per-org/registry setup)
  │
  └──► AUTH-5 (auth requirements — derive needed tokens from dist config)

AUTH-6 (Pre-flight checks in release/sync pipelines) ◄── AUTH-3, AUTH-5
```

## Tickets

| Ticket | Description | Depends on |
|--------|-------------|-----------|
| AUTH-2 | Credential registry: catalog of all known credential specs | — |
| AUTH-3 | `auth check`: validate all configured tokens, report missing/expired/wrong-scope | AUTH-2 |
| AUTH-4 | `auth setup`: guided credential setup per org/registry | AUTH-2 |
| AUTH-5 | `auth requirements`: derive needed credentials from workspace dist config | AUTH-2 |
| AUTH-6 | Pre-flight validation: check tokens before starting release/sync pipelines | AUTH-3, AUTH-5 |

## Phases

### Phase 1: Foundation (AUTH-2)
- Build the credential registry that catalogs all known credential types
- Maps DistChannel → required credentials
- Maps Forge → required credentials

### Phase 2: Validation (AUTH-3)
- `auth check` command hits validation endpoints for each configured token
- Reports: valid, invalid, missing, wrong scopes, expired
- GitHub: `GET /user` + parse `X-OAuth-Scopes` header
- Codeberg: `GET /api/v1/user`
- crates.io: `GET /api/v1/me`

### Phase 3: Setup (AUTH-4)
- `auth setup --org hypermemetic --forge github` walks through token creation
- Shows exactly which scopes to enable
- Provides the URL to create the token
- Validates after input

### Phase 4: Requirements (AUTH-5)
- Given a workspace with dist configs, enumerate all needed credentials
- `auth requirements --path .` shows what's needed and what's missing

### Phase 5: Pre-flight (AUTH-6)
- `build release` and `workspace sync` check credentials before starting work
- Fail fast with clear message instead of failing mid-pipeline

## Success Criteria

- `auth check` validates all tokens and reports scope issues
- `auth setup` guides user through obtaining correct tokens
- `auth requirements` derives what's needed from dist config
- `build release` fails fast if tokens are missing/invalid
- No more "3 repos deep before discovering 401"
