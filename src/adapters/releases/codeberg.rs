//! Codeberg/Gitea Releases API adapter
//!
//! Uses the Gitea Releases API for creating releases and uploading assets.
//! Auth: same `codeberg/{org}/token` PAT.
//!
//! Key difference from GitHub: asset uploads use multipart/form-data instead of
//! raw binary body.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{header, multipart, Client};
use std::sync::Arc;

use crate::auth::AuthProvider;
use super::{AssetInfo, ReleaseError, ReleaseInfo, ReleasePort, ReleaseResult};

// --- Gitea API response types ---

#[derive(Debug, serde::Deserialize)]
struct GiteaRelease {
    id: u64,
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    created_at: String,
    #[serde(default)]
    assets: Vec<GiteaAsset>,
}

#[derive(Debug, serde::Deserialize)]
struct GiteaAsset {
    id: u64,
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    browser_download_url: String,
    created_at: String,
}

impl GiteaRelease {
    fn into_release_info(self) -> ReleaseInfo {
        let created_at = DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let assets = self.assets.into_iter().map(|a| a.into_asset_info()).collect();

        ReleaseInfo {
            id: self.id,
            tag_name: self.tag_name,
            name: self.name,
            body: self.body,
            draft: self.draft,
            prerelease: self.prerelease,
            created_at,
            assets,
        }
    }
}

impl GiteaAsset {
    fn into_asset_info(self) -> AssetInfo {
        let created_at = DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        AssetInfo {
            id: self.id,
            name: self.name,
            size_bytes: self.size,
            // Gitea doesn't return content_type in list responses
            content_type: String::new(),
            download_url: self.browser_download_url,
            created_at,
        }
    }
}

// --- Create release request body ---

#[derive(serde::Serialize)]
struct CreateReleaseBody {
    tag_name: String,
    name: String,
    body: String,
    draft: bool,
    prerelease: bool,
}

pub struct CodebergReleaseAdapter {
    client: Client,
    auth: Arc<dyn AuthProvider>,
    api_url: String,
    org: String,
}

impl CodebergReleaseAdapter {
    pub fn new(auth: Arc<dyn AuthProvider>, org: impl Into<String>) -> ReleaseResult<Self> {
        let client = Client::builder()
            .user_agent("hyperforge/4.0")
            .build()
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        Ok(Self {
            client,
            auth,
            api_url: "https://codeberg.org/api/v1".to_string(),
            org: org.into(),
        })
    }

    async fn auth_headers(&self) -> ReleaseResult<header::HeaderMap> {
        let secret_path = format!("codeberg/{}/token", self.org);
        let token = self
            .auth
            .get_secret(&secret_path)
            .await
            .map_err(|e| ReleaseError::AuthFailed(e.to_string()))?
            .ok_or_else(|| {
                ReleaseError::AuthFailed(format!(
                    "No Codeberg token found for org: {}",
                    self.org
                ))
            })?;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("token {}", token))
                .map_err(|e| ReleaseError::AuthFailed(e.to_string()))?,
        );
        Ok(headers)
    }

    /// Check response for common auth/not-found errors and return appropriate ReleaseError
    fn check_error_status(status: reqwest::StatusCode, body: &str) -> Option<ReleaseError> {
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Some(ReleaseError::AuthFailed(format!(
                "Codeberg auth error: {}",
                body
            )));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Some(ReleaseError::NotFound(format!(
                "Codeberg resource not found: {}",
                body
            )));
        }
        if !status.is_success() {
            return Some(ReleaseError::ApiError(format!(
                "Codeberg API error {}: {}",
                status, body
            )));
        }
        None
    }
}

#[async_trait]
impl ReleasePort for CodebergReleaseAdapter {
    async fn create_release(
        &self,
        org: &str,
        repo: &str,
        tag: &str,
        name: &str,
        body: &str,
        draft: bool,
        prerelease: bool,
    ) -> ReleaseResult<ReleaseInfo> {
        let headers = self.auth_headers().await?;

        let url = format!("{}/repos/{}/{}/releases", self.api_url, org, repo);

        let req_body = CreateReleaseBody {
            tag_name: tag.to_string(),
            name: name.to_string(),
            body: body.to_string(),
            draft,
            prerelease,
        };

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .json(&req_body)
            .send()
            .await
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Self::check_error_status(status, &body)
                .unwrap_or_else(|| ReleaseError::ApiError(format!("HTTP {}: {}", status, body))));
        }

        let release: GiteaRelease = response
            .json()
            .await
            .map_err(|e| ReleaseError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(release.into_release_info())
    }

    async fn upload_asset(
        &self,
        org: &str,
        repo: &str,
        release_id: u64,
        filename: &str,
        content_type: &str,
        data: Vec<u8>,
    ) -> ReleaseResult<AssetInfo> {
        let headers = self.auth_headers().await?;

        // Codeberg/Gitea uses multipart/form-data for asset uploads
        let url = format!(
            "{}/repos/{}/{}/releases/{}/assets?name={}",
            self.api_url,
            org,
            repo,
            release_id,
            urlencoding::encode(filename)
        );

        let part = multipart::Part::bytes(data)
            .file_name(filename.to_string())
            .mime_str(content_type)
            .map_err(|e| ReleaseError::ApiError(format!("Invalid content type: {}", e)))?;

        let form = multipart::Form::new().part("attachment", part);

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .multipart(form)
            .send()
            .await
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Self::check_error_status(status, &body)
                .unwrap_or_else(|| ReleaseError::ApiError(format!("HTTP {}: {}", status, body))));
        }

        let asset: GiteaAsset = response
            .json()
            .await
            .map_err(|e| ReleaseError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(asset.into_asset_info())
    }

    async fn list_releases(&self, org: &str, repo: &str) -> ReleaseResult<Vec<ReleaseInfo>> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/repos/{}/{}/releases?limit=50",
            self.api_url, org, repo
        );

        let response = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Self::check_error_status(status, &body)
                .unwrap_or_else(|| ReleaseError::ApiError(format!("HTTP {}: {}", status, body))));
        }

        let releases: Vec<GiteaRelease> = response
            .json()
            .await
            .map_err(|e| ReleaseError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(releases.into_iter().map(|r| r.into_release_info()).collect())
    }

    async fn get_release_by_tag(
        &self,
        org: &str,
        repo: &str,
        tag: &str,
    ) -> ReleaseResult<Option<ReleaseInfo>> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/repos/{}/{}/releases/tags/{}",
            self.api_url, org, repo, tag
        );

        let response = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Self::check_error_status(status, &body)
                .unwrap_or_else(|| ReleaseError::ApiError(format!("HTTP {}: {}", status, body))));
        }

        let release: GiteaRelease = response
            .json()
            .await
            .map_err(|e| ReleaseError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(Some(release.into_release_info()))
    }

    async fn delete_release(&self, org: &str, repo: &str, release_id: u64) -> ReleaseResult<()> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/repos/{}/{}/releases/{}",
            self.api_url, org, repo, release_id
        );

        let response = self
            .client
            .delete(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Self::check_error_status(status, &body)
                .unwrap_or_else(|| ReleaseError::ApiError(format!("HTTP {}: {}", status, body))));
        }

        Ok(())
    }

    async fn list_assets(
        &self,
        org: &str,
        repo: &str,
        release_id: u64,
    ) -> ReleaseResult<Vec<AssetInfo>> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/repos/{}/{}/releases/{}/assets",
            self.api_url, org, repo, release_id
        );

        let response = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Self::check_error_status(status, &body)
                .unwrap_or_else(|| ReleaseError::ApiError(format!("HTTP {}: {}", status, body))));
        }

        let assets: Vec<GiteaAsset> = response
            .json()
            .await
            .map_err(|e| ReleaseError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(assets.into_iter().map(|a| a.into_asset_info()).collect())
    }

    async fn delete_asset(&self, org: &str, repo: &str, asset_id: u64) -> ReleaseResult<()> {
        let headers = self.auth_headers().await?;

        // Gitea supports DELETE /repos/{owner}/{repo}/releases/assets/{id} for direct asset deletion
        let url = format!(
            "{}/repos/{}/{}/releases/assets/{}",
            self.api_url, org, repo, asset_id
        );

        let response = self
            .client
            .delete(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Self::check_error_status(status, &body)
                .unwrap_or_else(|| ReleaseError::ApiError(format!("HTTP {}: {}", status, body))));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gitea_release() {
        let json = r#"{
            "id": 555,
            "tag_name": "v2.0.0",
            "name": "Version 2.0.0",
            "body": "Changes: Feature A, Fix B",
            "draft": true,
            "prerelease": false,
            "created_at": "2026-03-15T12:00:00Z",
            "assets": [
                {
                    "id": 100,
                    "name": "app-linux-x86_64.tar.gz",
                    "size": 8388608,
                    "browser_download_url": "https://codeberg.org/org/repo/releases/download/v2.0.0/app-linux-x86_64.tar.gz",
                    "created_at": "2026-03-15T12:05:00Z"
                }
            ]
        }"#;

        let release: GiteaRelease = serde_json::from_str(json).unwrap();
        let info = release.into_release_info();

        assert_eq!(info.id, 555);
        assert_eq!(info.tag_name, "v2.0.0");
        assert_eq!(info.name, "Version 2.0.0");
        assert!(info.draft);
        assert!(!info.prerelease);
        assert_eq!(info.assets.len(), 1);

        let asset = &info.assets[0];
        assert_eq!(asset.id, 100);
        assert_eq!(asset.name, "app-linux-x86_64.tar.gz");
        assert_eq!(asset.size_bytes, 8388608);
    }

    #[test]
    fn test_parse_gitea_release_minimal() {
        let json = r#"{
            "id": 1,
            "tag_name": "v0.0.1",
            "created_at": "2026-01-01T00:00:00Z"
        }"#;

        let release: GiteaRelease = serde_json::from_str(json).unwrap();
        let info = release.into_release_info();

        assert_eq!(info.id, 1);
        assert_eq!(info.tag_name, "v0.0.1");
        assert_eq!(info.name, "");
        assert_eq!(info.body, "");
        assert!(!info.draft);
        assert!(!info.prerelease);
        assert!(info.assets.is_empty());
    }

    #[test]
    fn test_parse_gitea_asset() {
        let json = r#"{
            "id": 77,
            "name": "checksums.txt",
            "size": 1024,
            "browser_download_url": "https://codeberg.org/org/repo/releases/download/v1.0.0/checksums.txt",
            "created_at": "2026-02-10T08:00:00Z"
        }"#;

        let asset: GiteaAsset = serde_json::from_str(json).unwrap();
        let info = asset.into_asset_info();

        assert_eq!(info.id, 77);
        assert_eq!(info.name, "checksums.txt");
        assert_eq!(info.size_bytes, 1024);
        assert!(info.download_url.contains("checksums.txt"));
    }
}
