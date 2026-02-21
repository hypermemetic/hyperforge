//! crates.io registry client.
//!
//! Queries crates.io API for published versions and shells out to
//! `cargo publish` for publishing.

use super::{PublishResult, PublishedVersion, RegistryClient};
use crate::build_system::BuildSystemKind;
use crate::hub::PackageRegistry;
use async_trait::async_trait;
use std::path::Path;

/// crates.io registry client
pub struct CratesIoClient {
    http: reqwest::Client,
}

impl CratesIoClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent("hyperforge/4.0 (https://github.com/juggernautlabs/hyperforge)")
            .build()
            .expect("failed to build HTTP client");
        Self { http }
    }
}

#[async_trait]
impl RegistryClient for CratesIoClient {
    fn build_system(&self) -> BuildSystemKind {
        BuildSystemKind::Cargo
    }

    fn registry_kind(&self) -> PackageRegistry {
        PackageRegistry::CratesIo
    }

    async fn published_version(&self, name: &str) -> anyhow::Result<Option<PublishedVersion>> {
        let url = format!("https://crates.io/api/v1/crates/{}", name);

        let resp = self.http.get(&url).send().await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let resp = resp.error_for_status()?;
        let body: serde_json::Value = resp.json().await?;

        // crates.io response: { "crate": { "max_stable_version": "1.2.3", ... } }
        let version = body
            .get("crate")
            .and_then(|c| c.get("max_stable_version"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        match version {
            Some(v) => Ok(Some(PublishedVersion {
                name: name.to_string(),
                version: v,
            })),
            None => Ok(None),
        }
    }

    async fn publish(
        &self,
        path: &Path,
        name: &str,
        dry_run: bool,
    ) -> anyhow::Result<PublishResult> {
        let mut args = vec!["publish", "--no-verify"];
        if dry_run {
            args.push("--dry-run");
        }

        let output = tokio::process::Command::new("cargo")
            .args(&args)
            .current_dir(path)
            .output()
            .await?;

        let success = output.status.success();
        let error = if !success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Some(format!("{}\n{}", stderr.trim(), stdout.trim()).trim().to_string())
        } else {
            None
        };

        // Read the current version from Cargo.toml to report
        let version = crate::build_system::cargo::cargo_package_version(path)
            .unwrap_or_else(|| "unknown".to_string());

        Ok(PublishResult {
            package_name: name.to_string(),
            version,
            success,
            error,
        })
    }
}
