# TRAK-IDP: Built-in Identity Provider

blocked_by: [TRAK-MVP]
unlocks: [TRAK-8]

## Scope

Built-in IDP as a cfg activation on the trak DynamicHub. Issues JWTs,
hashes passwords with argon2, validates tokens on WebSocket connect.
Provides tenant isolation for all facet operations.

## Architecture

```
trak DynamicHub
  ├── facet      (FacetHub)
  ├── discuss    (stub)
  ├── audit      (stub)
  ├── access     (stub)
  ├── collab     (stub)
  ├── refs       (stub)
  └── identity   (IdentityHub — built-in IDP)
```

The IdentityHub registers as a child on the same DynamicHub.
It implements `SessionValidator` so the transport layer validates
JWTs on every WebSocket connection.

## Data model

```sql
CREATE TABLE users (
    id TEXT PRIMARY KEY,            -- UUID
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,    -- argon2id
    display_name TEXT,
    email TEXT,
    roles TEXT NOT NULL DEFAULT '[]', -- JSON array
    tenant TEXT,                    -- tenant isolation key
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,            -- UUID
    user_id TEXT NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL,         -- sha256 of the key
    created_at TEXT NOT NULL,
    expires_at TEXT,                -- optional expiry
    last_used_at TEXT
);

CREATE TABLE refresh_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    token_hash TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);
```

## JWT structure

```json
{
  "sub": "user-uuid",
  "username": "ben",
  "roles": ["admin", "user"],
  "tenant": "hypermemetic",
  "iat": 1234567890,
  "exp": 1234571490
}
```

Signed with RS256. Key pair generated on first run, stored at
`~/.config/trak/jwt_key.pem` / `jwt_key.pub.pem`.

Access token TTL: 1 hour.
Refresh token TTL: 30 days.

## Wire surface

```
trak.identity.register       --username --password --display_name? --email? --tenant?
trak.identity.login           --username --password
trak.identity.refresh         --refresh_token
trak.identity.me                                        -- current user from JWT
trak.identity.create_api_key  --name --expires_in?
trak.identity.revoke_api_key  --key_id
trak.identity.list_api_keys
trak.identity.list_users                                -- admin only
trak.identity.update_user     --user_id --roles? --tenant? --display_name?
```

## SessionValidator implementation

```rust
impl SessionValidator for TrakAuth {
    async fn validate(&self, cookie_or_bearer: &str) -> Option<AuthContext> {
        // Try JWT first
        if let Ok(claims) = decode_jwt(cookie_or_bearer, &self.public_key) {
            return Some(AuthContext {
                user_id: claims.sub,
                session_id: claims.jti,
                roles: claims.roles,
                metadata: json!({ "tenant": claims.tenant, "username": claims.username }),
            });
        }
        // Try API key
        if let Ok(user) = self.validate_api_key(cookie_or_bearer).await {
            return Some(AuthContext { ... });
        }
        None
    }
}
```

## Tenant isolation in FacetHub

Every FacetStore query gains an implicit tenant filter:

```rust
// In FacetHub methods:
let tenant = auth.and_then(|a| a.get_metadata_string("tenant"));
let results = self.store.list_children(parent, tenant.as_deref(), ...).await?;
```

Facets created without auth → owner "anonymous", no tenant.
Facets created with auth → owner from user_id, tenant from JWT.

## Tests

### `test_register_and_login`
Register user. Login with credentials. Assert JWT returned with correct claims.

### `test_jwt_validation`
Login, use token to connect WebSocket. Assert AuthContext populated.

### `test_wrong_password`
Register, login with wrong password. Assert error.

### `test_refresh_flow`
Login, wait, refresh. Assert new access token.

### `test_api_key`
Create API key. Use it to authenticate. Assert works.

### `test_tenant_isolation`
Register user A (tenant=alpha), user B (tenant=beta).
A creates facet. B lists facets. Assert B cannot see A's facet.

### `test_admin_role`
Non-admin tries list_users. Assert error.
Admin tries list_users. Assert success.
