//! GitHub Releases API adapter
//!
//! Uses the GitHub REST API for creating releases and uploading assets.
//! Auth: same `github/{org}/token` PAT, needs `repo` scope for releases.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{header, Client};
use std::sync::Arc;

use crate::auth::AuthProvider;
use super::{AssetInfo, ReleaseError, ReleaseInfo, ReleasePort, ReleaseResult};

// --- GitHub API response types ---

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct GhRelease {
    id: u64,
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    created_at: String,
    #[serde(default)]
    upload_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, serde::Deserialize)]
struct GhAsset {
    id: u64,
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    browser_download_url: String,
    created_at: String,
}

impl GhRelease {
    fn into_release_info(self) -> ReleaseInfo {
        let created_at = DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let assets = self.assets.into_iter().map(|a| a.into_asset_info()).collect();

        ReleaseInfo {
            id: self.id,
            tag_name: self.tag_name,
            name: self.name.unwrap_or_default(),
            body: self.body.unwrap_or_default(),
            draft: self.draft,
            prerelease: self.prerelease,
            created_at,
            assets,
        }
    }
}

impl GhAsset {
    fn into_asset_info(self) -> AssetInfo {
        let created_at = DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        AssetInfo {
            id: self.id,
            name: self.name,
            size_bytes: self.size,
            content_type: self.content_type,
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

pub struct GitHubReleaseAdapter {
    client: Client,
    auth: Arc<dyn AuthProvider>,
    api_url: String,
    upload_url: String,
    org: String,
}

impl GitHubReleaseAdapter {
    pub fn new(auth: Arc<dyn AuthProvider>, org: impl Into<String>) -> ReleaseResult<Self> {
        let client = Client::builder()
            .user_agent("hyperforge/4.0")
            .build()
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        Ok(Self {
            client,
            auth,
            api_url: "https://api.github.com".to_string(),
            upload_url: "https://uploads.github.com".to_string(),
            org: org.into(),
        })
    }

    async fn auth_headers(&self) -> ReleaseResult<header::HeaderMap> {
        let secret_path = format!("github/{}/token", self.org);
        let token = self
            .auth
            .get_secret(&secret_path)
            .await
            .map_err(|e| ReleaseError::AuthFailed(e.to_string()))?
            .ok_or_else(|| {
                ReleaseError::AuthFailed(format!(
                    "No GitHub token found for org: {}",
                    self.org
                ))
            })?;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", token))
                .map_err(|e| ReleaseError::AuthFailed(e.to_string()))?,
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        Ok(headers)
    }

    /// Check response for common auth/not-found errors and return appropriate ReleaseError
    fn check_error_status(status: reqwest::StatusCode, body: &str) -> Option<ReleaseError> {
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Some(ReleaseError::AuthFailed(format!(
                "GitHub auth error: {} (token may need repo scope)",
                body
            )));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Some(ReleaseError::NotFound(format!(
                "GitHub resource not found: {}",
                body
            )));
        }
        if !status.is_success() {
            return Some(ReleaseError::ApiError(format!(
                "GitHub API error {}: {}",
                status, body
            )));
        }
        None
    }
}

#[async_trait]
impl ReleasePort for GitHubReleaseAdapter {
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

        let release: GhRelease = response
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

        // GitHub uses a separate upload host
        let url = format!(
            "{}/repos/{}/{}/releases/{}/assets?name={}",
            self.upload_url,
            org,
            repo,
            release_id,
            urlencoding::encode(filename)
        );

        let content_type_header = header::HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| header::HeaderValue::from_static("application/octet-stream"));

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .header(header::CONTENT_TYPE, content_type_header)
            .body(data)
            .send()
            .await
            .map_err(|e| ReleaseError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Self::check_error_status(status, &body)
                .unwrap_or_else(|| ReleaseError::ApiError(format!("HTTP {}: {}", status, body))));
        }

        let asset: GhAsset = response
            .json()
            .await
            .map_err(|e| ReleaseError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(asset.into_asset_info())
    }

    async fn list_releases(&self, org: &str, repo: &str) -> ReleaseResult<Vec<ReleaseInfo>> {
        let headers = self.auth_headers().await?;

        let url = format!(
            "{}/repos/{}/{}/releases?per_page=100",
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

        let releases: Vec<GhRelease> = response
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

        let release: GhRelease = response
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
            "{}/repos/{}/{}/releases/{}/assets?per_page=100",
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

        let assets: Vec<GhAsset> = response
            .json()
            .await
            .map_err(|e| ReleaseError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(assets.into_iter().map(|a| a.into_asset_info()).collect())
    }

    async fn delete_asset(&self, org: &str, repo: &str, asset_id: u64) -> ReleaseResult<()> {
        let headers = self.auth_headers().await?;

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
    fn test_parse_gh_release() {
        let json = r#"{
            "id": 12345,
            "tag_name": "v1.0.0",
            "name": "Release 1.0.0",
            "body": "First release",
            "draft": false,
            "prerelease": false,
            "created_at": "2026-01-15T10:00:00Z",
            "upload_url": "https://uploads.github.com/repos/org/repo/releases/12345/assets{?name,label}",
            "assets": [
                {
                    "id": 999,
                    "name": "binary-linux-amd64",
                    "size": 10485760,
                    "content_type": "application/octet-stream",
                    "browser_download_url": "https://github.com/org/repo/releases/download/v1.0.0/binary-linux-amd64",
                    "created_at": "2026-01-15T10:05:00Z"
                }
            ]
        }"#;

        let release: GhRelease = serde_json::from_str(json).unwrap();
        let info = release.into_release_info();

        assert_eq!(info.id, 12345);
        assert_eq!(info.tag_name, "v1.0.0");
        assert_eq!(info.name, "Release 1.0.0");
        assert_eq!(info.body, "First release");
        assert!(!info.draft);
        assert!(!info.prerelease);
        assert_eq!(info.assets.len(), 1);

        let asset = &info.assets[0];
        assert_eq!(asset.id, 999);
        assert_eq!(asset.name, "binary-linux-amd64");
        assert_eq!(asset.size_bytes, 10485760);
        assert_eq!(asset.content_type, "application/octet-stream");
    }

    #[test]
    fn test_parse_gh_release_minimal() {
        let json = r#"{
            "id": 1,
            "tag_name": "v0.1.0",
            "created_at": "2026-01-01T00:00:00Z",
            "upload_url": ""
        }"#;

        let release: GhRelease = serde_json::from_str(json).unwrap();
        let info = release.into_release_info();

        assert_eq!(info.id, 1);
        assert_eq!(info.tag_name, "v0.1.0");
        assert_eq!(info.name, "");
        assert_eq!(info.body, "");
        assert!(!info.draft);
        assert!(!info.prerelease);
        assert!(info.assets.is_empty());
    }

    #[test]
    fn test_parse_gh_asset() {
        let json = r#"{
            "id": 42,
            "name": "app.tar.gz",
            "size": 5242880,
            "content_type": "application/gzip",
            "browser_download_url": "https://github.com/org/repo/releases/download/v1.0.0/app.tar.gz",
            "created_at": "2026-03-20T14:30:00Z"
        }"#;

        let asset: GhAsset = serde_json::from_str(json).unwrap();
        let info = asset.into_asset_info();

        assert_eq!(info.id, 42);
        assert_eq!(info.name, "app.tar.gz");
        assert_eq!(info.size_bytes, 5242880);
        assert_eq!(info.content_type, "application/gzip");
        assert!(info.download_url.contains("app.tar.gz"));
    }
}
