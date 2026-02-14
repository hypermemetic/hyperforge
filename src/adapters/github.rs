//! GitHub adapter implementing ForgePort trait
//!
//! Uses the GitHub REST API v3 to manage repositories.

use async_trait::async_trait;
use reqwest::{Client, header, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::auth::AuthProvider;
use crate::types::{Forge, Repo, Visibility};
use super::{ForgeError, ForgePort, ForgeResult, ListResult};

/// GitHub API base URL
const GITHUB_API_URL: &str = "https://api.github.com";

/// GitHub repository response from API
#[derive(Debug, Deserialize)]
struct GitHubRepo {
    name: String,
    description: Option<String>,
    private: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    default_branch: Option<String>,
}

/// Request body for creating a repository
#[derive(Debug, Serialize)]
struct CreateRepoRequest {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    private: bool,
}

/// Request body for updating a repository
#[derive(Debug, Serialize)]
struct UpdateRepoRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    private: Option<bool>,
}

/// Request body for renaming a repository
#[derive(Debug, Serialize)]
struct RenameRepoRequest {
    name: String,
}

/// GitHub adapter for ForgePort trait
pub struct GitHubAdapter {
    client: Client,
    auth: Arc<dyn AuthProvider>,
    api_url: String,
    org: String,
}

impl GitHubAdapter {
    /// Create a new GitHubAdapter with the given auth provider
    pub fn new(auth: Arc<dyn AuthProvider>, org: impl Into<String>) -> ForgeResult<Self> {
        Self::with_api_url(auth, org, GITHUB_API_URL.to_string())
    }

    /// Create a new GitHubAdapter with a custom API URL (for testing)
    pub fn with_api_url(auth: Arc<dyn AuthProvider>, org: impl Into<String>, api_url: String) -> ForgeResult<Self> {
        let client = Client::builder()
            .user_agent("hyperforge/2.0")
            .build()
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        Ok(Self { client, auth, api_url, org: org.into() })
    }

    /// Get authorization headers with token from auth provider
    async fn auth_headers(&self) -> ForgeResult<header::HeaderMap> {
        // Construct secret path: github/{org}/token
        let secret_path = format!("github/{}/token", self.org);
        let token = self.auth.get_secret(&secret_path).await
            .map_err(|e| ForgeError::AuthenticationFailed { message: e.to_string() })?
            .ok_or_else(|| ForgeError::AuthenticationFailed {
                message: format!("No GitHub token found for org: {}", self.org),
            })?;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", token))
                .map_err(|e| ForgeError::AuthenticationFailed { message: e.to_string() })?,
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            header::HeaderValue::from_static("2022-11-28"),
        );

        Ok(headers)
    }

    /// Convert GitHub API response to our Repo type
    fn to_repo(gh_repo: GitHubRepo) -> Repo {
        Repo {
            name: gh_repo.name,
            description: gh_repo.description,
            visibility: if gh_repo.private {
                Visibility::Private
            } else {
                Visibility::Public
            },
            origin: Forge::GitHub,
            mirrors: Vec::new(),
            protected: gh_repo.archived,
            staged_for_deletion: false,
        }
    }

    /// Parse GitHub's Link header to find the last page number.
    /// GitHub uses: `<url?page=N>; rel="last"` format.
    /// Returns None if there is no "last" link (i.e., only one page).
    fn parse_last_page(response: &Response) -> Option<u32> {
        let link_header = response.headers().get("link")?.to_str().ok()?;
        for part in link_header.split(',') {
            if part.contains("rel=\"last\"") {
                // Extract page=N from the URL portion
                let url_part = part.split(';').next()?;
                let url = url_part.trim().trim_start_matches('<').trim_end_matches('>');
                // Find page= parameter
                for param in url.split('?').nth(1)?.split('&') {
                    if let Some(val) = param.strip_prefix("page=") {
                        return val.parse::<u32>().ok();
                    }
                }
            }
        }
        None
    }

    /// Fetch all pages of GitHub repos from a paginated endpoint.
    /// The first response is provided (already fetched serially).
    /// Remaining pages are fetched in parallel.
    async fn fetch_all_pages(
        &self,
        first_response: Response,
        base_url: &str,
    ) -> ForgeResult<Vec<Repo>> {
        let last_page = Self::parse_last_page(&first_response);

        let first_repos: Vec<GitHubRepo> = first_response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        let mut all_repos: Vec<Repo> = first_repos.into_iter().map(Self::to_repo).collect();

        // If there's no last page or it's page 1, we're done
        let last_page = match last_page {
            Some(lp) if lp > 1 => lp,
            _ => return Ok(all_repos),
        };

        // Fetch remaining pages in parallel (pages 2..=last_page)
        let headers = self.auth_headers().await?;
        let mut join_set = JoinSet::new();

        // Limit concurrency to 10 pages at a time
        let separator = if base_url.contains('?') { '&' } else { '?' };
        for page in 2..=last_page {
            let client = self.client.clone();
            let hdrs = headers.clone();
            let url = format!("{}{separator}page={page}", base_url);

            join_set.spawn(async move {
                let response = client.get(&url)
                    .headers(hdrs)
                    .send()
                    .await
                    .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ForgeError::ApiError(format!(
                        "GitHub API error {}: {}", status, body
                    )));
                }

                let repos: Vec<GitHubRepo> = response.json().await
                    .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

                Ok(repos)
            });

            // Limit concurrency: if we have 10 in-flight, wait for one to complete
            if join_set.len() >= 10 {
                if let Some(result) = join_set.join_next().await {
                    let repos = result
                        .map_err(|e| ForgeError::ApiError(format!("Task join error: {}", e)))??;
                    all_repos.extend(repos.into_iter().map(Self::to_repo));
                }
            }
        }

        // Collect remaining results
        while let Some(result) = join_set.join_next().await {
            let repos = result
                .map_err(|e| ForgeError::ApiError(format!("Task join error: {}", e)))??;
            all_repos.extend(repos.into_iter().map(Self::to_repo));
        }

        Ok(all_repos)
    }
}

#[async_trait]
impl ForgePort for GitHubAdapter {
    async fn list_repos(&self, org: &str) -> ForgeResult<Vec<Repo>> {
        let headers = self.auth_headers().await?;
        let base_url = format!("{}/orgs/{}/repos?per_page=100", self.api_url, org);

        let response = self.client.get(&base_url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            // Try user repos if org not found
            return self.list_user_repos(org).await;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        self.fetch_all_pages(response, &base_url).await
    }

    async fn get_repo(&self, org: &str, name: &str) -> ForgeResult<Repo> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/repos/{}/{}", self.api_url, org, name);

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ForgeError::RepoNotFound { name: name.to_string() });
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        let gh_repo: GitHubRepo = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(Self::to_repo(gh_repo))
    }

    async fn create_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/orgs/{}/repos", self.api_url, org);

        let request = CreateRepoRequest {
            name: repo.name.clone(),
            description: repo.description.clone(),
            private: repo.visibility == Visibility::Private,
        };

        let response = self.client.post(&url)
            .headers(headers.clone())
            .json(&request)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        // If org create fails with 404, try user create
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return self.create_user_repo(repo).await;
        }

        if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            let body = response.text().await.unwrap_or_default();
            if body.contains("name already exists") {
                return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
            }
            return Err(ForgeError::ApiError(format!("GitHub API error: {}", body)));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        Ok(())
    }

    async fn update_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/repos/{}/{}", self.api_url, org, repo.name);

        let request = UpdateRepoRequest {
            description: repo.description.clone(),
            private: Some(repo.visibility == Visibility::Private),
        };

        let response = self.client.patch(&url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ForgeError::RepoNotFound { name: repo.name.clone() });
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        Ok(())
    }

    async fn delete_repo(&self, org: &str, name: &str) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/repos/{}/{}", self.api_url, org, name);

        let response = self.client.delete(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            // Already deleted, treat as success
            return Ok(());
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        Ok(())
    }

    async fn set_default_branch(&self, org: &str, name: &str, branch: &str) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/repos/{}/{}", self.api_url, org, name);

        let body = serde_json::json!({ "default_branch": branch });

        let response = self.client.patch(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ForgeError::RepoNotFound { name: name.to_string() });
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        Ok(())
    }

    async fn rename_repo(&self, org: &str, old_name: &str, new_name: &str) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/repos/{}/{}", self.api_url, org, old_name);

        let request = RenameRepoRequest {
            name: new_name.to_string(),
        };

        let response = self.client.patch(&url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ForgeError::RepoNotFound { name: old_name.to_string() });
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        Ok(())
    }

    async fn list_repos_incremental(
        &self, org: &str, etag: Option<String>,
    ) -> ForgeResult<ListResult> {
        let mut headers = self.auth_headers().await?;
        if let Some(ref etag_value) = etag {
            headers.insert(
                header::IF_NONE_MATCH,
                header::HeaderValue::from_str(etag_value)
                    .map_err(|e| ForgeError::ApiError(format!("Invalid ETag value: {}", e)))?,
            );
        }

        let url = format!("{}/orgs/{}/repos?per_page=100", self.api_url, org);

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        // 304 Not Modified — nothing changed since the provided ETag
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(ListResult {
                repos: None,
                etag: etag,
                modified: false,
            });
        }

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            // Fallback to user repos (no ETag support for fallback)
            let repos = self.list_user_repos(org).await?;
            return Ok(ListResult {
                repos: Some(repos),
                etag: None,
                modified: true,
            });
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        let new_etag = response.headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let repos = self.fetch_all_pages(response, &url).await?;

        Ok(ListResult {
            repos: Some(repos),
            etag: new_etag,
            modified: true,
        })
    }
}

impl GitHubAdapter {
    /// List repos for a user (fallback when org doesn't exist)
    async fn list_user_repos(&self, username: &str) -> ForgeResult<Vec<Repo>> {
        let headers = self.auth_headers().await?;
        let base_url = format!("{}/users/{}/repos?per_page=100", self.api_url, username);

        let response = self.client.get(&base_url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        self.fetch_all_pages(response, &base_url).await
    }

    /// Create repo under authenticated user (fallback when org doesn't exist)
    async fn create_user_repo(&self, repo: &Repo) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/user/repos", self.api_url);

        let request = CreateRepoRequest {
            name: repo.name.clone(),
            description: repo.description.clone(),
            private: repo.visibility == Visibility::Private,
        };

        let response = self.client.post(&url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            let body = response.text().await.unwrap_or_default();
            if body.contains("name already exists") {
                return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
            }
            return Err(ForgeError::ApiError(format!("GitHub API error: {}", body)));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitHub API error {}: {}", status, body
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock auth provider for testing
    struct MockAuthProvider {
        token: Option<String>,
    }

    impl MockAuthProvider {
        fn with_token(token: &str) -> Self {
            Self { token: Some(token.to_string()) }
        }

        fn without_token() -> Self {
            Self { token: None }
        }
    }

    #[async_trait]
    impl AuthProvider for MockAuthProvider {
        async fn get_secret(&self, _key: &str) -> anyhow::Result<Option<String>> {
            Ok(self.token.clone())
        }
    }

    #[test]
    fn test_to_repo_public() {
        let gh_repo = GitHubRepo {
            name: "test-repo".to_string(),
            description: Some("A test repo".to_string()),
            private: false,
            archived: false,
            default_branch: None,
        };

        let repo = GitHubAdapter::to_repo(gh_repo);
        assert_eq!(repo.name, "test-repo");
        assert_eq!(repo.description, Some("A test repo".to_string()));
        assert_eq!(repo.visibility, Visibility::Public);
        assert_eq!(repo.origin, Forge::GitHub);
        assert!(!repo.protected);
    }

    #[test]
    fn test_to_repo_private_archived() {
        let gh_repo = GitHubRepo {
            name: "private-repo".to_string(),
            description: None,
            private: true,
            archived: true,
            default_branch: None,
        };

        let repo = GitHubAdapter::to_repo(gh_repo);
        assert_eq!(repo.visibility, Visibility::Private);
        assert!(repo.protected); // archived maps to protected
    }

    /*
    #[tokio::test]
    async fn test_auth_headers_missing_token() {
        let auth = Arc::new(MockAuthProvider::without_token());
        let adapter = GitHubAdapter::new(auth).unwrap();

        let result = adapter.auth_headers().await;
        assert!(matches!(result, Err(ForgeError::AuthenticationFailed { .. })));
    }
    */

    /*
    #[tokio::test]
    async fn test_auth_headers_with_token() {
        let auth = Arc::new(MockAuthProvider::with_token("ghp_test123"));
        let adapter = GitHubAdapter::new(auth).unwrap();

        let headers = adapter.auth_headers().await.unwrap();
        assert!(headers.contains_key(header::AUTHORIZATION));
        assert!(headers.contains_key(header::ACCEPT));
    }
    */
}
