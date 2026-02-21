//! Hackage registry client.
//!
//! Queries Hackage API for published versions and shells out to
//! `cabal upload --publish` for publishing. Fetches auth token from
//! cabal's `password-command` config or `HACKAGE_TOKEN` env var.

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

    /// Resolve the Hackage auth token. Tries in order:
    /// 1. `HACKAGE_TOKEN` env var
    /// 2. `password-command` from `~/.cabal/config`
    async fn resolve_token(&self) -> Option<String> {
        // 1. Env var
        if let Ok(token) = std::env::var("HACKAGE_TOKEN") {
            if !token.is_empty() {
                return Some(token);
            }
        }

        // 2. Parse password-command from cabal config
        let config_path = dirs::home_dir()?.join(".cabal/config");
        let config = tokio::fs::read_to_string(&config_path).await.ok()?;
        let cmd = config
            .lines()
            .find(|l| l.starts_with("password-command:"))?
            .trim_start_matches("password-command:")
            .trim();

        if cmd.is_empty() {
            return None;
        }

        let output = tokio::process::Command::new("sh")
            .args(["-c", cmd])
            .output()
            .await
            .ok()?;

        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }

        None
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
                .args(["sdist", "--ignore-project"])
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

        // Resolve auth token before publishing
        let token = self.resolve_token().await;

        // Step 1: build source distribution tarball
        let sdist_output = tokio::process::Command::new("cabal")
            .args(["sdist", "--ignore-project"])
            .current_dir(path)
            .output()
            .await?;

        if !sdist_output.status.success() {
            let version = crate::build_system::cabal::cabal_package_version(path)
                .unwrap_or_else(|| "unknown".to_string());
            return Ok(PublishResult {
                package_name: name.to_string(),
                version,
                success: false,
                error: Some(format!(
                    "cabal sdist failed: {}",
                    String::from_utf8_lossy(&sdist_output.stderr).trim()
                )),
            });
        }

        // Find the tarball path from sdist output (last line is the path)
        let sdist_stdout = String::from_utf8_lossy(&sdist_output.stdout);
        let tarball = sdist_stdout
            .lines()
            .rev()
            .find(|l| l.ends_with(".tar.gz"))
            .map(|l| l.trim().to_string());

        let tarball = match tarball {
            Some(t) => t,
            None => {
                let version = crate::build_system::cabal::cabal_package_version(path)
                    .unwrap_or_else(|| "unknown".to_string());
                return Ok(PublishResult {
                    package_name: name.to_string(),
                    version,
                    success: false,
                    error: Some("cabal sdist did not produce a tarball path".to_string()),
                });
            }
        };

        // Step 2: upload the tarball with --token to bypass interactive username prompt
        let mut upload_args = vec!["upload".to_string(), "--publish".to_string()];
        if let Some(ref tok) = token {
            upload_args.push(format!("--token={}", tok));
        }
        upload_args.push(tarball);
        let output = tokio::process::Command::new("cabal")
            .args(&upload_args)
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
