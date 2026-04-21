//! Codeberg Container Registry adapter
//!
//! Uses the Gitea Packages API for listing and deleting container packages.
//! Auth: same `codeberg/{org}/token` PAT.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{header, Client};
use std::sync::Arc;

use crate::auth::AuthProvider;
use super::{ImageTag, PackageInfo, RegistryError, RegistryPort, RegistryResult};

/// Gitea package list response
#[derive(Debug, serde::Deserialize)]
struct GiteaPackage {
    name: String,
    version: String,
    created_at: String,
}

pub struct CodebergRegistryAdapter {
    client: Client,
    auth: Arc<dyn AuthProvider>,
    api_url: String,
    org: String,
}

impl CodebergRegistryAdapter {
    pub fn new(auth: Arc<dyn AuthProvider>, org: impl Into<String>) -> RegistryResult<Self> {
        let client = Client::builder()
            .user_agent("hyperforge/4.0")
            .build()
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        Ok(Self {
            client,
            auth,
            api_url: "https://codeberg.org/api/v1".to_string(),
            org: org.into(),
        })
    }

    async fn auth_headers(&self) -> RegistryResult<header::HeaderMap> {
        let secret_path = format!("codeberg/{}/token", self.org);
        let token = self.auth.get_secret(&secret_path).await
            .map_err(|e| RegistryError::AuthFailed(e.to_string()))?
            .ok_or_else(|| RegistryError::AuthFailed(
                format!("No Codeberg token found for org: {}", self.org),
            ))?;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("token {token}"))
                .map_err(|e| RegistryError::AuthFailed(e.to_string()))?,
        );
        Ok(headers)
    }
}

#[async_trait]
impl RegistryPort for CodebergRegistryAdapter {
    async fn list_packages(&self, org: &str) -> RegistryResult<Vec<PackageInfo>> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/packages/{}?type=container&limit=50",
            self.api_url, org
        );

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::AuthFailed(format!(
                "Codeberg auth error: {body}"
            )));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::ApiError(format!(
                "Codeberg API error {status}: {body}"
            )));
        }

        let packages: Vec<GiteaPackage> = response.json().await
            .map_err(|e| RegistryError::ApiError(format!("Failed to parse: {e}")))?;

        // Group by name, count versions
        let mut by_name: std::collections::HashMap<String, Vec<GiteaPackage>> = std::collections::HashMap::new();
        for pkg in packages {
            by_name.entry(pkg.name.clone()).or_default().push(pkg);
        }

        Ok(by_name.into_iter().map(|(name, versions)| {
            let latest = versions.first().map(|v| v.version.clone());
            let created_at = versions.first()
                .and_then(|v| DateTime::parse_from_rfc3339(&v.created_at).ok())
                .map(|dt| dt.with_timezone(&Utc));
            PackageInfo {
                name,
                tag_count: versions.len(),
                latest_tag: latest,
                created_at,
            }
        }).collect())
    }

    async fn list_images(&self, org: &str, repo: &str) -> RegistryResult<Vec<ImageTag>> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/packages/{}?type=container&q={}&limit=50",
            self.api_url, org, repo
        );

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::ApiError(format!(
                "Codeberg API error {status}: {body}"
            )));
        }

        let packages: Vec<GiteaPackage> = response.json().await
            .map_err(|e| RegistryError::ApiError(format!("Failed to parse: {e}")))?;

        Ok(packages.into_iter()
            .filter(|p| p.name == repo)
            .map(|p| {
                let created_at = DateTime::parse_from_rfc3339(&p.created_at).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc));
                ImageTag {
                    tag: p.version,
                    digest: String::new(),
                    size_bytes: 0,
                    created_at,
                }
            })
            .collect())
    }

    async fn delete_image(&self, org: &str, repo: &str, tag: &str) -> RegistryResult<()> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/packages/{}/container/{}/{}",
            self.api_url, org, repo, tag
        );

        let response = self.client.delete(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| RegistryError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RegistryError::NotFound(format!("Tag '{tag}' not found")));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RegistryError::ApiError(format!(
                "Codeberg API error {status}: {body}"
            )));
        }

        Ok(())
    }

    fn registry_host(&self) -> &'static str {
        "codeberg.org"
    }
}
