//! GitHub adapter implementing ForgePort trait
//!
//! Uses the GitHub REST API v3 to manage repositories.

use async_trait::async_trait;
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::AuthProvider;
use crate::types::{Forge, Repo, Visibility};
use super::{ForgeError, ForgePort, ForgeResult};

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
        }
    }
}

#[async_trait]
impl ForgePort for GitHubAdapter {
    async fn list_repos(&self, org: &str) -> ForgeResult<Vec<Repo>> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/orgs/{}/repos?per_page=100", self.api_url, org);

        let response = self.client.get(&url)
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

        let gh_repos: Vec<GitHubRepo> = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(gh_repos.into_iter().map(Self::to_repo).collect())
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
}

impl GitHubAdapter {
    /// List repos for a user (fallback when org doesn't exist)
    async fn list_user_repos(&self, username: &str) -> ForgeResult<Vec<Repo>> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/users/{}/repos?per_page=100", self.api_url, username);

        let response = self.client.get(&url)
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

        let gh_repos: Vec<GitHubRepo> = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(gh_repos.into_iter().map(Self::to_repo).collect())
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
        };

        let repo = GitHubAdapter::to_repo(gh_repo);
        assert_eq!(repo.visibility, Visibility::Private);
        assert!(repo.protected); // archived maps to protected
    }

    #[tokio::test]
    async fn test_auth_headers_missing_token() {
        let auth = Arc::new(MockAuthProvider::without_token());
        let adapter = GitHubAdapter::new(auth).unwrap();

        let result = adapter.auth_headers().await;
        assert!(matches!(result, Err(ForgeError::AuthenticationFailed { .. })));
    }

    #[tokio::test]
    async fn test_auth_headers_with_token() {
        let auth = Arc::new(MockAuthProvider::with_token("ghp_test123"));
        let adapter = GitHubAdapter::new(auth).unwrap();

        let headers = adapter.auth_headers().await.unwrap();
        assert!(headers.contains_key(header::AUTHORIZATION));
        assert!(headers.contains_key(header::ACCEPT));
    }
}
