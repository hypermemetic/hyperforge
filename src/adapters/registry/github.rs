//! GitHub Container Registry (ghcr.io) adapter
//!
//! Uses the GitHub REST API for listing and deleting package versions.
//! Auth: same `github/{org}/token` PAT, needs `read:packages` scope.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{header, Client};
use std::sync::Arc;

use crate::auth::AuthProvider;
use super::{ImageTag, PackageInfo, RegistryError, RegistryPort, RegistryResult};

/// GitHub packages list response
#[derive(Debug, serde::Deserialize)]
struct GhPackage {
    name: String,
    package_type: String,
    created_at: String,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    html_url: String,
}

/// GitHub packages API response for a container version
#[derive(Debug, serde::Deserialize)]
struct GhPackageVersion {
    id: u64,
    name: String, // digest
    #[serde(default)]
    metadata: Option<GhPackageMetadata>,
    created_at: String,
}

#[derive(Debug, serde::Deserialize)]
struct GhPackageMetadata {
    container: Option<GhContainerMetadata>,
}

#[derive(Debug, serde::Deserialize)]
struct GhContainerMetadata {
    #[serde(default)]
    tags: Vec<String>,
}

pub struct GitHubRegistryAdapter {
    client: Client,
    auth: Arc<dyn AuthProvider>,
    api_url: String,
    org: String,
}

impl GitHubRegistryAdapter {
    pub fn new(auth: Arc<dyn AuthProvider>, org: impl Into<String>) -> RegistryResult<Self> {
        let client = Client::builder()
            .user_agent("hyperforge/4.0")
            .build()
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        Ok(Self {
            client,
            auth,
            api_url: "https://api.github.com".to_string(),
            org: org.into(),
        })
    }

    async fn auth_headers(&self) -> RegistryResult<header::HeaderMap> {
        // Try packages_token first (classic PAT with read:packages), fall back to token
        let packages_path = format!("github/{}/packages_token", self.org);
        let default_path = format!("github/{}/token", self.org);

        let token = match self.auth.get_secret(&packages_path).await {
            Ok(Some(t)) => t,
            _ => self.auth.get_secret(&default_path).await
                .map_err(|e| RegistryError::AuthFailed(e.to_string()))?
                .ok_or_else(|| RegistryError::AuthFailed(
                    format!("No GitHub token found for org: {} (tried packages_token and token)", self.org),
                ))?,
        };

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", token))
                .map_err(|e| RegistryError::AuthFailed(e.to_string()))?,
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        Ok(headers)
    }
}

#[async_trait]
impl RegistryPort for GitHubRegistryAdapter {
    async fn list_packages(&self, org: &str) -> RegistryResult<Vec<PackageInfo>> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/orgs/{}/packages?package_type=container&per_page=100",
            self.api_url, org
        );

        let response = self.client.get(&url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        // Fall back to user endpoint
        let response = if response.status() == reqwest::StatusCode::NOT_FOUND {
            let user_url = format!(
                "{}/users/{}/packages?package_type=container&per_page=100",
                self.api_url, org
            );
            self.client.get(&user_url)
                .headers(headers)
                .send()
                .await
                .map_err(|e| RegistryError::NetworkError(e.to_string()))?
        } else {
            response
        };

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::AuthFailed(format!(
                "Token may need read:packages scope: {}", body
            )));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        let packages: Vec<GhPackage> = response.json().await
            .map_err(|e| RegistryError::ApiError(format!("Failed to parse: {}", e)))?;

        Ok(packages.into_iter()
            .filter(|p| p.package_type == "container")
            .map(|p| {
                let created_at = DateTime::parse_from_rfc3339(&p.created_at)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok();
                PackageInfo {
                    name: p.name,
                    tag_count: 0, // populated by list_images if needed
                    latest_tag: None,
                    created_at,
                }
            })
            .collect())
    }

    async fn list_images(&self, org: &str, repo: &str) -> RegistryResult<Vec<ImageTag>> {
        let headers = self.auth_headers().await?;

        // Try org endpoint first, fall back to user
        let url = format!(
            "{}/orgs/{}/packages/container/{}/versions?per_page=100",
            self.api_url, org, repo
        );

        let response = self.client.get(&url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        let response = if response.status() == reqwest::StatusCode::NOT_FOUND {
            // Fall back to user endpoint
            let user_url = format!(
                "{}/users/{}/packages/container/{}/versions?per_page=100",
                self.api_url, org, repo
            );
            self.client.get(&user_url)
                .headers(headers)
                .send()
                .await
                .map_err(|e| RegistryError::NetworkError(e.to_string()))?
        } else {
            response
        };

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new()); // No package exists yet
        }

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::AuthFailed(format!(
                "GitHub API {}: {} (token may need read:packages scope)",
                "auth error", body
            )));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        let versions: Vec<GhPackageVersion> = response.json().await
            .map_err(|e| RegistryError::ApiError(format!("Failed to parse response: {}", e)))?;

        let mut tags = Vec::new();
        for version in versions {
            let container_tags = version.metadata
                .and_then(|m| m.container)
                .map(|c| c.tags)
                .unwrap_or_default();

            let created_at = DateTime::parse_from_rfc3339(&version.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            if container_tags.is_empty() {
                // Untagged image — still report it with digest as tag
                tags.push(ImageTag {
                    tag: format!("<untagged:{}>", &version.name[..12.min(version.name.len())]),
                    digest: version.name,
                    size_bytes: 0, // GitHub API doesn't return size in versions endpoint
                    created_at,
                });
            } else {
                for tag in container_tags {
                    tags.push(ImageTag {
                        tag,
                        digest: version.name.clone(),
                        size_bytes: 0,
                        created_at,
                    });
                }
            }
        }

        Ok(tags)
    }

    async fn delete_image(&self, org: &str, repo: &str, tag: &str) -> RegistryResult<()> {
        let headers = self.auth_headers().await?;

        // First, find the version ID for this tag
        let images = self.list_images(org, repo).await?;
        let target = images.iter().find(|img| img.tag == tag)
            .ok_or_else(|| RegistryError::NotFound(format!("Tag '{}' not found", tag)))?;

        // We need the version ID — re-fetch versions to get it
        let url = format!(
            "{}/orgs/{}/packages/container/{}/versions?per_page=100",
            self.api_url, org, repo
        );

        let response = self.client.get(&url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        // Fall back to user endpoint
        let response = if response.status() == reqwest::StatusCode::NOT_FOUND {
            let user_url = format!(
                "{}/users/{}/packages/container/{}/versions?per_page=100",
                self.api_url, org, repo
            );
            self.client.get(&user_url)
                .headers(headers.clone())
                .send()
                .await
                .map_err(|e| RegistryError::NetworkError(e.to_string()))?
        } else {
            response
        };

        let versions: Vec<GhPackageVersion> = response.json().await
            .map_err(|e| RegistryError::ApiError(format!("Failed to parse: {}", e)))?;

        let version_id = versions.iter()
            .find(|v| v.name == target.digest)
            .map(|v| v.id)
            .ok_or_else(|| RegistryError::NotFound(format!("Version for tag '{}' not found", tag)))?;

        // Delete the version
        let delete_url = format!(
            "{}/orgs/{}/packages/container/{}/versions/{}",
            self.api_url, org, repo, version_id
        );

        let response = self.client.delete(&delete_url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::ApiError(format!(
                "Failed to delete version {}: {} {}", version_id, status, body
            )));
        }

        Ok(())
    }

    fn registry_host(&self) -> &str {
        "ghcr.io"
    }
}
