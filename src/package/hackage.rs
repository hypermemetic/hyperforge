//! Hackage registry client.
//!
//! Queries Hackage API for published versions and shells out to
//! `cabal upload --publish` for publishing.

use super::{PublishResult, PublishedVersion, RegistryClient};
use crate::build_system::BuildSystemKind;
use crate::hub::PackageRegistry;
use async_trait::async_trait;
use std::path::Path;

/// Hackage registry client
pub struct HackageClient {
    http: reqwest::Client,
}

impl HackageClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent("hyperforge/4.0")
            .build()
            .expect("failed to build HTTP client");
        Self { http }
    }
}

#[async_trait]
impl RegistryClient for HackageClient {
    fn build_system(&self) -> BuildSystemKind {
        BuildSystemKind::Cabal
    }

    fn registry_kind(&self) -> PackageRegistry {
        PackageRegistry::Hackage
    }

    async fn published_version(&self, name: &str) -> anyhow::Result<Option<PublishedVersion>> {
        let url = format!(
            "https://hackage.haskell.org/package/{}/preferred",
            name
        );

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let resp = resp.error_for_status()?;
        let body: serde_json::Value = resp.json().await?;

        // Hackage response: { "normal-version": ["1.2.3", "1.2.2", ...], ... }
        let version = body
            .get("normal-version")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
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
        if dry_run {
            // Dry-run: build source distribution to validate
            let output = tokio::process::Command::new("cabal")
                .args(["sdist"])
                .current_dir(path)
                .output()
                .await?;

            let success = output.status.success();
            let error = if !success {
                Some(String::from_utf8_lossy(&output.stderr).trim().to_string())
            } else {
                None
            };

            let version = crate::build_system::cabal::cabal_package_version(path)
                .unwrap_or_else(|| "unknown".to_string());

            return Ok(PublishResult {
                package_name: name.to_string(),
                version,
                success,
                error,
            });
        }

        let output = tokio::process::Command::new("cabal")
            .args(["upload", "--publish"])
            .current_dir(path)
            .output()
            .await?;

        let success = output.status.success();
        let error = if !success {
            Some(String::from_utf8_lossy(&output.stderr).trim().to_string())
        } else {
            None
        };

        let version = crate::build_system::cabal::cabal_package_version(path)
            .unwrap_or_else(|| "unknown".to_string());

        Ok(PublishResult {
            package_name: name.to_string(),
            version,
            success,
            error,
        })
    }
}
