# FORGE-6: Update `hub.rs` — event variant, methods, `get_org_defaults()`

blocked_by: [FORGE-2, FORGE-3, FORGE-4, FORGE-5]
unlocks: [FORGE-7]

## Scope

Update the main hub file to use the flat `forges` model throughout. This is the largest ticket — touches event types, multiple RPC methods, and adds the `get_org_defaults()` helper.

## File

`src/hub.rs`

## Changes

### 1. `HyperforgeEvent::Repo` variant (~line 34)

Replace `origin: String` + `mirrors: Vec<String>` with `forges: Vec<String>`.

Update all construction sites (repos_list ~231, repos_create ~340, repos_import ~945):
```rust
forges: repo.forges.iter().map(|f| f.as_str().to_string()).collect(),
```

### 2. `HyperforgeEvent::SyncSummary`

Replace `to_delete` field with `remote_only`.

### 3. `get_org_defaults()` helper

```rust
async fn get_org_defaults(&self, org: &str) -> Option<OrgDefaults> {
    let path = self.config_dir.join("orgs").join(org).join("defaults.yaml");
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    serde_yaml::from_str(&content).ok()
}
```

Best-effort: if file missing or malformed, returns `None`.

### 4. `repos_create` — forges as optional param with org defaults (~line 260)

Change params from `origin: String, mirrors: Option<String>` to `forges: Option<String>`.

When `forges` is `None`, resolve from org defaults:
```rust
let forge_list: Vec<Forge> = if let Some(forges_str) = forges {
    parse_forge_list(&forges_str)
} else {
    match hub.get_org_defaults(&org).await {
        Some(defaults) if !defaults.forges.is_empty() => defaults.forges,
        _ => {
            yield HyperforgeEvent::Error { message: "No forges specified and no org defaults found".into() };
            return;
        }
    }
};
```

Visibility also falls back to org defaults when not specified.

### 5. `repos_import` — merge forges when repo exists (~line 825)

Change from skip-existing to merge:
```rust
if exists {
    match local.get_repo(&org, &repo.name).await {
        Ok(mut existing) => {
            if existing.forges.contains(&source_forge) {
                skipped += 1;
            } else {
                existing.merge_forge(source_forge);
                local.update_repo(&org, &existing).await?;
                merged += 1;
            }
        }
        Err(_) => { skipped += 1; }
    }
    continue;
}
```

Add `merged` counter. Update summary message.

### 6. `workspace_diff` — forge collection (~line 1025)

Replace `repo.all_forges()` with direct `repo.forges` iteration.

Update operation string from `"delete"` to `"remote_only"`.

### 7. `workspace_register` — no origin/mirrors split (~line 3515)

Replace origin-from-first / mirrors-from-rest logic with:
```rust
let forges: Vec<Forge> = config.forges.iter()
    .filter_map(|f| HyperforgeConfig::parse_forge(f))
    .collect();
let repo = Repo::new(repo_name, forges).with_visibility(config.visibility.clone());
```

### 8. `workspace_sync_status` — dynamic forge discovery (~line 1129)

Replace hardcoded `[github, codeberg]` with forge discovery from LocalForge repos.

### 9. `repos_rename` — `all_forges()` to `forges` (~line 525)

Replace `repo.all_forges()` with `repo.forges.clone()`.

### 10. `workspace_sync` — use per-repo forge membership (~line 1228)

Filter repos to only those containing the target forge before syncing.

## Acceptance criteria

- No references to `origin`, `mirrors`, `all_forges()`, `to_delete` in hub.rs
- `repos_create` works without `--forges` when org defaults exist
- `repos_import` merges forge into existing repos instead of skipping
- `workspace_diff` shows `remote_only` instead of `delete`
- `workspace_register` uses flat forges list
- `workspace_sync_status` discovers forges dynamically
