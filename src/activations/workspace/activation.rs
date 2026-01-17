//! Thin WorkspaceActivation - delegates to WorkspaceService.

use async_stream::stream;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use hub_core::plexus::{Activation, ChildRouter, PlexusError, PlexusStream, ChildSummary};
use hub_macro::hub_methods;
use super::service::WorkspaceService;
use crate::events::{OrgEvent, WorkspaceEvent};
use crate::storage::HyperforgePaths;
use crate::types::ResolutionSource;

pub struct WorkspaceActivation { service: WorkspaceService }

impl WorkspaceActivation {
    pub fn new(paths: Arc<HyperforgePaths>) -> Self { Self { service: WorkspaceService::new(paths) } }
}

#[hub_methods(namespace = "workspace", version = "1.0.0", description = "Workspace binding management", crate_path = "hub_core", hub)]
impl WorkspaceActivation {
    #[hub_method(description = "List all workspace bindings")]
    pub async fn list(&self) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let result = self.service.list_bindings().await;
        stream! { match result { Ok(b) => yield WorkspaceEvent::Listed { bindings: b }, Err(e) => yield WorkspaceEvent::Error { message: e } } }
    }

    #[hub_method(description = "Show current workspace resolution", params(path = "Path to check"))]
    pub async fn show(&self, path: String) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let p = PathBuf::from(&path);
        let result = self.service.resolve_workspace(&p).await;
        stream! {
            match result {
                Ok(Some(r)) => yield WorkspaceEvent::Resolved { org_name: r.org_name, source: if r.workspace_path == p { ResolutionSource::Default } else { ResolutionSource::WorkspaceBinding { path: r.workspace_path } }, path: p },
                Ok(None) => yield WorkspaceEvent::NotBound { path: p },
                Err(e) => yield WorkspaceEvent::Error { message: e },
            }
        }
    }

    #[hub_method(description = "Bind a directory to an organization", params(path = "Directory path", org_name = "Organization", auto_create = "Scan for git repos"))]
    pub async fn bind(&self, path: String, org_name: String, auto_create: Option<bool>) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let bp = PathBuf::from(&path);
        let svc = WorkspaceService::new(self.service.paths().clone());
        stream! {
            if let Err(e) = svc.bind(bp.clone(), org_name.clone()).await { yield WorkspaceEvent::Error { message: e }; return; }
            yield WorkspaceEvent::Bound { path: bp.clone(), org_name: org_name.clone() };
            if auto_create.unwrap_or(false) {
                if let Ok(repos) = svc.discover_git_repos(&bp).await {
                    if !repos.is_empty() {
                        yield WorkspaceEvent::ReposDiscovered { path: bp.clone(), repos: repos.clone() };
                        for rn in repos {
                            match svc.stage_repo(&org_name, rn.clone(), None).await {
                                Ok(()) => yield WorkspaceEvent::RepoStaged { repo_name: rn },
                                Err(e) => yield WorkspaceEvent::Error { message: format!("Failed to stage: {}", e) },
                            }
                        }
                    }
                }
            }
        }
    }

    #[hub_method(description = "Remove a workspace binding", params(path = "Directory path to unbind"))]
    pub async fn unbind(&self, path: String) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let p = PathBuf::from(&path);
        let result = self.service.unbind(&p).await;
        stream! { match result { Ok(true) => yield WorkspaceEvent::Unbound { path: p }, Ok(false) => yield WorkspaceEvent::NotBound { path: p }, Err(e) => yield WorkspaceEvent::Error { message: e } } }
    }

    #[hub_method(description = "Show diff for workspace orgs", params(path = "Path to check"))]
    pub async fn diff(&self, path: String) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let p = PathBuf::from(&path);
        let svc = WorkspaceService::new(self.service.paths().clone());
        stream! {
            let cfg = match svc.load_config().await { Ok(c) => c, Err(e) => { yield WorkspaceEvent::Error { message: e }; return; } };
            let r = match svc.resolve_workspace(&p).await { Ok(Some(r)) if !r.bound_orgs.is_empty() => r, _ => { yield WorkspaceEvent::NotBound { path: p }; return; } };
            yield WorkspaceEvent::PreviewStarted { workspace_path: r.workspace_path.clone(), org_count: r.bound_orgs.len() };
            let (mut tc, mut tup, mut td, mut tu) = (0, 0, 0, 0);
            for org in &r.bound_orgs {
                // Use process_org_sync with auto_yes=false for dry-run with detailed output
                for ev in svc.process_org_sync(&cfg, org, &r.workspace_path, false).await {
                    match &ev { WorkspaceEvent::OrgPreviewComplete { to_create, to_update, to_delete, unchanged, .. } => { tc += to_create; tup += to_update; td += to_delete; tu += unchanged; } _ => {} }
                    yield ev;
                }
            }
            yield WorkspaceEvent::PreviewComplete { workspace_path: r.workspace_path, total_to_create: tc, total_to_update: tup, total_to_delete: td, total_unchanged: tu };
        }
    }

    #[hub_method(description = "Import repos for workspace orgs", params(path = "Path", include_private = "Include private"))]
    pub async fn import(&self, path: String, include_private: Option<bool>) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let cwd = PathBuf::from(&path);
        let svc = WorkspaceService::new(self.service.paths().clone());
        let inc = include_private.unwrap_or(false);
        stream! {
            let r = match svc.resolve_workspace(&cwd).await { Ok(Some(r)) => r, _ => { yield WorkspaceEvent::NotBound { path: cwd }; return; } };
            yield WorkspaceEvent::ImportStarted { workspace_path: r.workspace_path.clone(), org_count: r.bound_orgs.len() };
            let (mut ti, mut ts, mut te) = (0, 0, 0);
            let oa = svc.org_activation();
            for org in r.bound_orgs {
                yield WorkspaceEvent::OrgImportStarted { org_name: org.clone() };
                let (mut i, mut s, mut e) = (0, 0, 0);
                let mut st = std::pin::pin!(oa.import(org.clone(), Some(inc), None).await);
                while let Some(ev) = st.next().await {
                    match ev { OrgEvent::ImportComplete { imported_count, skipped_count, .. } => { i = imported_count; s = skipped_count; } OrgEvent::Error { message } => { e += 1; yield WorkspaceEvent::Error { message: format!("{}: {}", org, message) }; } _ => {} }
                }
                yield WorkspaceEvent::OrgImportComplete { org_name: org, imported: i, skipped: s, errors: e };
                ti += i; ts += s; te += e;
            }
            yield WorkspaceEvent::ImportComplete { total_imported: ti, total_skipped: ts, total_errors: te };
        }
    }

    #[hub_method(description = "Clone all repos for workspace orgs", params(path = "Path"))]
    pub async fn clone_all(&self, path: String) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let cwd = PathBuf::from(&path);
        let svc = WorkspaceService::new(self.service.paths().clone());
        stream! {
            let cfg = match svc.load_config().await { Ok(c) => c, Err(e) => { yield WorkspaceEvent::Error { message: e }; return; } };
            let r = match svc.resolve_workspace(&cwd).await { Ok(Some(r)) => r, _ => { yield WorkspaceEvent::NotBound { path: cwd }; return; } };
            yield WorkspaceEvent::CloneAllStarted { org_count: r.bound_orgs.len() };
            let (mut tc, mut ts, mut tf) = (0, 0, 0);
            for org in &r.bound_orgs {
                yield WorkspaceEvent::OrgCloneAllStarted { org_name: org.clone() };
                let (mut c, mut s, mut f) = (0, 0, 0);
                if let (Some(oc), Ok(repos)) = (cfg.get_org(org), svc.org_storage(org).load_repos().await) {
                    let oc = oc.clone(); let df = oc.forges.all_forges();
                    for (n, rc) in &repos.repos {
                        if rc.delete { continue; }
                        match svc.clone_repo(&r.workspace_path, n, &oc, org, rc.forges.as_ref().unwrap_or(&df)).await { Ok(true) => c += 1, Ok(false) => s += 1, Err(_) => f += 1 }
                    }
                } else { f += 1; }
                yield WorkspaceEvent::OrgCloneAllComplete { org_name: org.clone(), cloned: c, skipped: s, failed: f };
                tc += c; ts += s; tf += f;
            }
            yield WorkspaceEvent::CloneAllComplete { total_cloned: tc, total_skipped: ts, total_failed: tf };
        }
    }

    #[hub_method(description = "Sync repos for workspace orgs", params(path = "Path", yes = "Skip confirmation"))]
    pub async fn sync(&self, path: String, yes: Option<bool>) -> impl Stream<Item = WorkspaceEvent> + Send + 'static {
        let p = PathBuf::from(&path);
        let svc = WorkspaceService::new(self.service.paths().clone());
        let ay = yes.unwrap_or(false);
        stream! {
            let cfg = match svc.load_config().await { Ok(c) => c, Err(e) => { yield WorkspaceEvent::Error { message: e }; return; } };
            let r = match svc.resolve_workspace(&p).await { Ok(Some(r)) if !r.bound_orgs.is_empty() => r, _ => { yield WorkspaceEvent::NotBound { path: p }; return; } };
            if ay { yield WorkspaceEvent::SyncStarted { workspace_path: r.workspace_path.clone(), org_count: r.bound_orgs.len() }; }
            else { yield WorkspaceEvent::PreviewStarted { workspace_path: r.workspace_path.clone(), org_count: r.bound_orgs.len() }; }
            let (mut ts, mut tu, mut tf, mut tc, mut tup, mut td) = (0, 0, 0, 0, 0, 0);
            for org in &r.bound_orgs {
                for ev in svc.process_org_sync(&cfg, org, &r.workspace_path, ay).await {
                    match &ev { WorkspaceEvent::OrgSyncComplete { synced, unchanged, failed, .. } => { ts += synced; tu += unchanged; tf += failed; } WorkspaceEvent::OrgPreviewComplete { to_create, to_update, to_delete, unchanged, .. } => { tc += to_create; tup += to_update; td += to_delete; tu += unchanged; } _ => {} }
                    yield ev;
                }
            }
            if ay { yield WorkspaceEvent::SyncComplete { workspace_path: r.workspace_path, total_synced: ts, total_unchanged: tu, total_failed: tf }; }
            else { yield WorkspaceEvent::PreviewComplete { workspace_path: r.workspace_path, total_to_create: tc, total_to_update: tup, total_to_delete: td, total_unchanged: tu }; }
        }
    }

    pub fn plugin_children(&self) -> Vec<ChildSummary> {
        vec![
            ChildSummary {
                namespace: "repos".into(),
                description: "Repository management for workspace".into(),
                hash: "repos".into(),
            },
        ]
    }
}

#[async_trait]
impl ChildRouter for WorkspaceActivation {
    fn router_namespace(&self) -> &str { "workspace" }
    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> { Activation::call(self, method, params).await }
    async fn get_child(&self, name: &str) -> Option<Box<dyn ChildRouter>> {
        match name {
            "repos" => Some(Box::new(super::WorkspaceReposRouter::new(self.service.paths().clone()))),
            _ => None,
        }
    }
}
