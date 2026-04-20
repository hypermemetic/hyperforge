//! Integration tests for hyperforge hub

use futures::StreamExt;
use plexus_core::plexus::{DynamicHub, PlexusStreamItem};
use hyperforge::{
    HyperforgeEvent, HyperforgeHub, OrgAddFailureReason, OrgUpdateFailureReason, OrgUpdateOp,
};
use std::sync::Arc;
use tempfile::TempDir;

/// Drain a routed stream into a vector of `HyperforgeEvent`s, ignoring any
/// non-`Data` items.
async fn drain_hyperforge_events(hub: &DynamicHub, method: &str, params: serde_json::Value) -> Vec<HyperforgeEvent> {
    let mut stream = hub.route(method, params, None).await.expect("route call");
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        if let PlexusStreamItem::Data { content, .. } = item {
            if let Ok(event) = serde_json::from_value::<HyperforgeEvent>(content) {
                events.push(event);
            }
        }
    }
    events
}

/// Build a hub rooted at a tempdir config directory. Returns (hub, tempdir).
fn test_hub() -> (Arc<DynamicHub>, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let hyperforge = HyperforgeHub::new_with_config_dir(tmp.path().to_path_buf());
    let hub = Arc::new(DynamicHub::new("testhub").register(hyperforge));
    (hub, tmp)
}

#[tokio::test]
async fn test_hyperforge_as_plugin() {
    // Create hyperforge activation
    let hyperforge = HyperforgeHub::new();

    // Register in DynamicHub
    let hub = Arc::new(DynamicHub::new("testhub").register(hyperforge));

    // Call hyperforge.status via DynamicHub routing
    let mut stream = hub.route("hyperforge.status", serde_json::json!({}), None).await.unwrap();

    let mut found_status = false;
    while let Some(item) = stream.next().await {
        if let plexus_core::plexus::PlexusStreamItem::Data { content, .. } = item {
            if let Ok(HyperforgeEvent::Status { version, description }) =
                serde_json::from_value::<HyperforgeEvent>(content)
            {
                assert_eq!(version, env!("CARGO_PKG_VERSION"));
                assert!(description.contains("FORGE4"));
                found_status = true;
            }
        }
    }

    assert!(found_status, "Should have received status event");
}

#[tokio::test]
async fn test_dynamic_hub_lists_hyperforge() {
    // Create hyperforge activation
    let hyperforge = HyperforgeHub::new();

    // Register in DynamicHub
    let hub = DynamicHub::new("testhub").register(hyperforge);

    // Check that hyperforge is listed in activations
    let activations = hub.list_activations_info();
    let hyperforge_activation = activations
        .iter()
        .find(|a| a.namespace == "hyperforge")
        .expect("hyperforge should be listed in activations");

    assert_eq!(hyperforge_activation.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(hyperforge_activation.description, "Multi-forge repository management");

    // Check that methods are listed
    assert!(hyperforge_activation.methods.contains(&"status".to_string()));
}

// ---------------------------------------------------------------------------
// orgs_add (ORGS-2)
// ---------------------------------------------------------------------------

/// Scrape the list of org names reported by `orgs_list`. Returns only the
/// org names in the order listed (one message line per org, format
/// "  <org> — workspace: ..., forges: [...]").
async fn list_orgs(hub: &DynamicHub) -> Vec<String> {
    let events = drain_hyperforge_events(hub, "hyperforge.orgs_list", serde_json::json!({})).await;
    let mut out = Vec::new();
    for ev in events {
        if let HyperforgeEvent::Info { message } = ev {
            let trimmed = message.trim_start();
            // Skip the terminating summary line "N org(s) configured."
            if trimmed.contains(" org(s) configured") {
                continue;
            }
            if let Some(dash_idx) = trimmed.find(" — ") {
                let name = trimmed[..dash_idx].trim();
                if !name.is_empty() {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

#[tokio::test]
async fn test_orgs_add_success_persists_and_shows_in_list() {
    let (hub, _tmp) = test_hub();

    // dry-run first — must NOT persist
    let preview = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "newtest",
        "ssh": { "github": "/tmp/gh_key" },
        "dry_run": true,
    })).await;

    let preview_ev = preview.iter().find(|e| matches!(e, HyperforgeEvent::OrgAdded { .. }))
        .expect("dry-run should emit OrgAdded");
    match preview_ev {
        HyperforgeEvent::OrgAdded { org, dry_run } => {
            assert_eq!(org, "newtest");
            assert!(*dry_run, "dry_run event should be flagged");
        }
        _ => unreachable!(),
    }

    assert!(!list_orgs(&hub).await.contains(&"newtest".to_string()),
        "dry-run must not persist an org");

    // now commit it
    let committed = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "newtest",
        "ssh": { "github": "/tmp/gh_key" },
        "workspace_path": "/tmp/newtest-ws",
    })).await;

    let created = committed.iter().find(|e| matches!(e, HyperforgeEvent::OrgAdded { .. }))
        .expect("commit should emit OrgAdded");
    match created {
        HyperforgeEvent::OrgAdded { org, dry_run } => {
            assert_eq!(org, "newtest");
            assert!(!*dry_run, "non-dry-run OrgAdded should have dry_run=false");
        }
        _ => unreachable!(),
    }

    let orgs = list_orgs(&hub).await;
    assert!(orgs.contains(&"newtest".to_string()),
        "orgs_list should include the newly added org, got {:?}", orgs);
}

#[tokio::test]
async fn test_orgs_add_already_exists_is_distinguishable_and_file_untouched() {
    let (hub, tmp) = test_hub();

    // Create once
    drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "dup",
        "ssh": { "github": "/tmp/original" },
    })).await;

    // Capture mtime + content.
    let config_path = tmp.path().join("orgs").join("dup.toml");
    let original_mtime = std::fs::metadata(&config_path).unwrap().modified().unwrap();
    let original_content = std::fs::read_to_string(&config_path).unwrap();

    // Sleep just long enough that any accidental overwrite would be visible.
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Attempt again — must fail with AlreadyExists.
    let retry = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "dup",
        "ssh": { "github": "/tmp/different" },
    })).await;

    let failure = retry.iter().find_map(|e| match e {
        HyperforgeEvent::OrgAddFailed { org, reason, .. } => Some((org.clone(), reason.clone())),
        _ => None,
    }).expect("second orgs_add should emit OrgAddFailed");

    assert_eq!(failure.0, "dup");
    assert_eq!(failure.1, OrgAddFailureReason::AlreadyExists,
        "duplicate must be distinguishable from invalid-name via reason");

    // mtime and content must be unchanged — no silent overwrite.
    let new_mtime = std::fs::metadata(&config_path).unwrap().modified().unwrap();
    let new_content = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(original_mtime, new_mtime, "refused orgs_add must not touch the existing file");
    assert_eq!(original_content, new_content, "refused orgs_add must leave contents intact");
}

#[tokio::test]
async fn test_orgs_add_invalid_name_is_distinguishable() {
    let (hub, _tmp) = test_hub();

    // Path traversal
    let traversal = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "../escape",
        "dry_run": true,
    })).await;
    let traversal_failure = traversal.iter().find_map(|e| match e {
        HyperforgeEvent::OrgAddFailed { reason, .. } => Some(reason.clone()),
        _ => None,
    }).expect("path-traversal org name must fail");
    assert_eq!(traversal_failure, OrgAddFailureReason::InvalidName);

    // Slash
    let slash = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "foo/bar",
    })).await;
    let slash_failure = slash.iter().find_map(|e| match e {
        HyperforgeEvent::OrgAddFailed { reason, .. } => Some(reason.clone()),
        _ => None,
    }).expect("slash org name must fail");
    assert_eq!(slash_failure, OrgAddFailureReason::InvalidName);

    // Empty
    let empty = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "",
    })).await;
    let empty_failure = empty.iter().find_map(|e| match e {
        HyperforgeEvent::OrgAddFailed { reason, .. } => Some(reason.clone()),
        _ => None,
    }).expect("empty org name must fail");
    assert_eq!(empty_failure, OrgAddFailureReason::InvalidName);

    // Nothing ever persisted
    assert!(list_orgs(&hub).await.is_empty(),
        "no org should have been created by any of the invalid attempts");
}

#[tokio::test]
async fn test_orgs_add_invalid_name_distinct_from_already_exists() {
    // A single invoker must be able to tell the two failure cases apart by
    // inspecting only the event stream — no source-code diffing.
    let (hub, _tmp) = test_hub();

    // Create a valid org first.
    drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({ "org": "already" })).await;

    let already = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({ "org": "already" })).await;
    let invalid = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({ "org": "../bad" })).await;

    let already_reason = already.iter().find_map(|e| match e {
        HyperforgeEvent::OrgAddFailed { reason, .. } => Some(reason.clone()),
        _ => None,
    }).expect("duplicate must emit OrgAddFailed");
    let invalid_reason = invalid.iter().find_map(|e| match e {
        HyperforgeEvent::OrgAddFailed { reason, .. } => Some(reason.clone()),
        _ => None,
    }).expect("invalid-name must emit OrgAddFailed");

    assert_ne!(already_reason, invalid_reason,
        "callers must be able to distinguish already-exists from invalid-name");
    assert_eq!(already_reason, OrgAddFailureReason::AlreadyExists);
    assert_eq!(invalid_reason, OrgAddFailureReason::InvalidName);
}

#[tokio::test]
async fn test_orgs_add_round_trip_preserves_ssh_and_workspace_path() {
    let (hub, _tmp) = test_hub();

    // Provide values on all three known forges and a workspace_path.
    drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "rt",
        "ssh": {
            "github": "/tmp/gh",
            "codeberg": "/tmp/cb",
            "gitlab": "/tmp/gl",
        },
        "workspace_path": "/tmp/rt-ws",
    })).await;

    let list = drain_hyperforge_events(&hub, "hyperforge.orgs_list", serde_json::json!({})).await;
    let org_line = list.iter().find_map(|e| match e {
        HyperforgeEvent::Info { message } if message.contains("rt ") => Some(message.clone()),
        _ => None,
    }).expect("orgs_list should report rt");

    assert!(org_line.contains("/tmp/rt-ws"),
        "workspace_path must round-trip through orgs_list, got: {}", org_line);
    // All three forges must appear in the ssh listing
    assert!(org_line.contains("github"), "{}", org_line);
    assert!(org_line.contains("codeberg"), "{}", org_line);
    assert!(org_line.contains("gitlab"), "{}", org_line);
}

#[tokio::test]
async fn test_orgs_add_ssh_omitted_creates_empty_ssh_map() {
    let (hub, _tmp) = test_hub();

    let events = drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "noshh",
    })).await;
    assert!(events.iter().any(|e| matches!(e, HyperforgeEvent::OrgAdded { org, dry_run: false } if org == "noshh")));

    assert!(list_orgs(&hub).await.contains(&"noshh".to_string()));
}

// ---------------------------------------------------------------------------
// orgs_update (ORGS-3)
// ---------------------------------------------------------------------------

/// Read the current OrgConfig for `org` from disk, under the hub's config dir.
fn read_org_toml(tmp: &TempDir, org: &str) -> toml::Value {
    let p = tmp.path().join("orgs").join(format!("{}.toml", org));
    let raw = std::fs::read_to_string(&p).unwrap_or_default();
    toml::from_str(&raw).unwrap_or(toml::Value::Table(Default::default()))
}

/// Extract the `ssh.<forge>` value for an org, if present.
fn ssh_value(tmp: &TempDir, org: &str, forge: &str) -> Option<String> {
    read_org_toml(tmp, org)
        .get("ssh")
        .and_then(|v| v.get(forge))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract the top-level workspace_path of an org, if present.
fn ws_path_value(tmp: &TempDir, org: &str) -> Option<String> {
    read_org_toml(tmp, org)
        .get("workspace_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Seed an org named `upd` with only github ssh (acceptance-criterion #1 setup).
async fn seed_upd_org(hub: &DynamicHub) {
    drain_hyperforge_events(hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "upd",
        "ssh": { "github": "/tmp/gh_initial" },
    })).await;
}

#[tokio::test]
async fn test_orgs_update_not_found_is_distinguishable() {
    let (hub, _tmp) = test_hub();

    let events = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "ghost",
        "ssh": { "github": "/tmp/gh" },
    })).await;

    let failure = events.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdateFailed { org, reason, .. } => Some((org.clone(), reason.clone())),
        _ => None,
    }).expect("missing-org update should emit OrgUpdateFailed");

    assert_eq!(failure.0, "ghost");
    assert_eq!(failure.1, OrgUpdateFailureReason::NotFound);
}

#[tokio::test]
async fn test_orgs_update_no_fields_is_distinguishable() {
    let (hub, _tmp) = test_hub();
    seed_upd_org(&hub).await;

    let events = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
    })).await;

    let failure = events.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdateFailed { reason, .. } => Some(reason.clone()),
        _ => None,
    }).expect("no-fields update should emit OrgUpdateFailed");
    assert_eq!(failure, OrgUpdateFailureReason::NoFieldsToUpdate,
        "no-fields must be distinguishable from not-found");
}

#[tokio::test]
async fn test_orgs_update_not_found_distinct_from_no_fields() {
    let (hub, _tmp) = test_hub();
    seed_upd_org(&hub).await;

    let not_found = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "ghost",
        "ssh": { "github": "/tmp/gh" },
    })).await;
    let no_fields = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
    })).await;

    let nf_reason = not_found.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdateFailed { reason, .. } => Some(reason.clone()),
        _ => None,
    }).unwrap();
    let nf2_reason = no_fields.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdateFailed { reason, .. } => Some(reason.clone()),
        _ => None,
    }).unwrap();

    assert_ne!(nf_reason, nf2_reason,
        "callers must be able to distinguish not-found from no-fields-to-update");
}

#[tokio::test]
async fn test_orgs_update_dry_run_does_not_persist() {
    let (hub, tmp) = test_hub();
    seed_upd_org(&hub).await;

    let before = read_org_toml(&tmp, "upd");

    let events = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
        "ssh": { "codeberg": "/tmp/cb_new" },
        "dry_run": true,
    })).await;

    let success = events.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdated { dry_run, operations, .. } => Some((*dry_run, operations.clone())),
        _ => None,
    }).expect("dry-run should emit OrgUpdated");
    assert!(success.0, "dry_run flag must be true");
    assert_eq!(success.1, vec![OrgUpdateOp::SshMerged]);

    let after = read_org_toml(&tmp, "upd");
    assert_eq!(before, after, "dry-run must not mutate the on-disk TOML");
}

#[tokio::test]
async fn test_orgs_update_default_merge_preserves_other_fields() {
    let (hub, tmp) = test_hub();
    // Seed with github and codeberg
    drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "upd",
        "ssh": { "github": "/tmp/gh_old", "codeberg": "/tmp/cb_old" },
    })).await;

    // Update only github
    let events = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
        "ssh": { "github": "/tmp/gh_NEW" },
    })).await;
    assert!(events.iter().any(|e| matches!(e, HyperforgeEvent::OrgUpdated {
        operations, dry_run: false, ..
    } if operations == &vec![OrgUpdateOp::SshMerged])));

    // github updated, codeberg untouched
    assert_eq!(ssh_value(&tmp, "upd", "github").as_deref(), Some("/tmp/gh_NEW"));
    assert_eq!(ssh_value(&tmp, "upd", "codeberg").as_deref(), Some("/tmp/cb_old"));
}

#[tokio::test]
async fn test_orgs_update_replace_wipes_untouched_fields() {
    let (hub, tmp) = test_hub();
    // Seed with both github and codeberg
    drain_hyperforge_events(&hub, "hyperforge.orgs_add", serde_json::json!({
        "org": "upd",
        "ssh": { "github": "/tmp/gh", "codeberg": "/tmp/cb" },
    })).await;

    // Replace with empty ssh record (ssh passed, all fields None)
    let events = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
        "ssh": {},
        "replace": true,
    })).await;

    let ok = events.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdated { operations, dry_run: false, .. } => Some(operations.clone()),
        _ => None,
    }).expect("replace should succeed");
    assert!(ok.contains(&OrgUpdateOp::SshReplaced),
        "replace must surface SshReplaced, not SshMerged, got {:?}", ok);

    assert_eq!(ssh_value(&tmp, "upd", "github"), None, "github must be wiped");
    assert_eq!(ssh_value(&tmp, "upd", "codeberg"), None, "codeberg must be wiped");
}

#[tokio::test]
async fn test_orgs_update_workspace_path_three_intents() {
    let (hub, tmp) = test_hub();
    seed_upd_org(&hub).await;

    // Intent: SET
    let set = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
        "workspace_path": "/tmp/x",
    })).await;
    let set_ops = set.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdated { operations, .. } => Some(operations.clone()),
        _ => None,
    }).unwrap();
    assert!(set_ops.contains(&OrgUpdateOp::WorkspacePathSet));
    assert_eq!(ws_path_value(&tmp, "upd").as_deref(), Some("/tmp/x"));

    // Intent: KEEP — ssh mutation with workspace_path omitted.
    let keep = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
        "ssh": { "github": "/tmp/keep" },
    })).await;
    // Must NOT contain a WorkspacePath* op.
    let keep_ops = keep.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdated { operations, .. } => Some(operations.clone()),
        _ => None,
    }).unwrap();
    assert!(!keep_ops.contains(&OrgUpdateOp::WorkspacePathSet));
    assert!(!keep_ops.contains(&OrgUpdateOp::WorkspacePathCleared));
    assert_eq!(ws_path_value(&tmp, "upd").as_deref(), Some("/tmp/x"),
        "omitted workspace_path must preserve the prior value");

    // Intent: CLEAR — empty string.
    let clear = drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
        "workspace_path": "",
    })).await;
    let clear_ops = clear.iter().find_map(|e| match e {
        HyperforgeEvent::OrgUpdated { operations, .. } => Some(operations.clone()),
        _ => None,
    }).unwrap();
    assert!(clear_ops.contains(&OrgUpdateOp::WorkspacePathCleared));
    assert_eq!(ws_path_value(&tmp, "upd"), None,
        "clear intent must leave workspace_path absent from persisted TOML");
}

#[tokio::test]
async fn test_orgs_update_round_trip_through_orgs_list() {
    let (hub, _tmp) = test_hub();
    seed_upd_org(&hub).await;

    // Add codeberg (merge), set a workspace_path, all in one call
    drain_hyperforge_events(&hub, "hyperforge.orgs_update", serde_json::json!({
        "org": "upd",
        "ssh": { "codeberg": "/tmp/cb_rt" },
        "workspace_path": "/tmp/upd_ws",
    })).await;

    // orgs_list must surface both forges + the new workspace_path.
    let list = drain_hyperforge_events(&hub, "hyperforge.orgs_list", serde_json::json!({})).await;
    let line = list.iter().find_map(|e| match e {
        HyperforgeEvent::Info { message } if message.contains("upd ") => Some(message.clone()),
        _ => None,
    }).expect("orgs_list must include upd");

    assert!(line.contains("/tmp/upd_ws"), "workspace_path missing from list: {}", line);
    assert!(line.contains("github"), "github forge missing from list: {}", line);
    assert!(line.contains("codeberg"), "codeberg forge missing from list: {}", line);
}
