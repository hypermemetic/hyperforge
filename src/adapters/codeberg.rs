//! Codeberg adapter implementing ForgePort trait
//!
//! Uses the Gitea/Forgejo API v1 (Codeberg runs Forgejo).

use async_trait::async_trait;
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::AuthProvider;
use crate::types::{Forge, Repo, Visibility};
use super::{ForgeError, ForgePort, ForgeResult};

/// Codeberg API base URL
const CODEBERG_API_URL: &str = "https://codeberg.org/api/v1";

/// Codeberg/Gitea repository response from API
#[derive(Debug, Deserialize)]
struct CodebergRepo {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_init: Option<bool>,
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

/// Request body for setting default branch
#[derive(Debug, Serialize)]
struct SetDefaultBranchRequest {
    default_branch: String,
}

use crate::types::AccountType;

/// Codeberg adapter for ForgePort trait
pub struct CodebergAdapter {
    client: Client,
    auth: Arc<dyn AuthProvider>,
    api_url: String,
    org: String,
    account_type: AccountType,
}

impl CodebergAdapter {
    /// Create a new CodebergAdapter with the given auth provider (defaults to User account type)
    pub fn new(auth: Arc<dyn AuthProvider>, org: impl Into<String>) -> ForgeResult<Self> {
        Self::with_options(auth, org, CODEBERG_API_URL.to_string(), AccountType::User)
    }

    /// Create a new CodebergAdapter with explicit account type
    pub fn with_account_type(auth: Arc<dyn AuthProvider>, org: impl Into<String>, account_type: AccountType) -> ForgeResult<Self> {
        Self::with_options(auth, org, CODEBERG_API_URL.to_string(), account_type)
    }

    /// Create a new CodebergAdapter with a custom API URL (for testing)
    pub fn with_api_url(auth: Arc<dyn AuthProvider>, org: impl Into<String>, api_url: String) -> ForgeResult<Self> {
        Self::with_options(auth, org, api_url, AccountType::User)
    }

    /// Create a new CodebergAdapter with all options
    pub fn with_options(auth: Arc<dyn AuthProvider>, org: impl Into<String>, api_url: String, account_type: AccountType) -> ForgeResult<Self> {
        let client = Client::builder()
            .user_agent("hyperforge/2.0")
            .build()
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        Ok(Self { client, auth, api_url, org: org.into(), account_type })
    }

    /// Check if this is a user account (vs organization)
    fn is_user(&self) -> bool {
        self.account_type == AccountType::User
    }

    /// Get authorization headers with token from auth provider
    async fn auth_headers(&self) -> ForgeResult<header::HeaderMap> {
        // Construct secret path: codeberg/{org}/token
        let secret_path = format!("codeberg/{}/token", self.org);
        let token = self.auth.get_secret(&secret_path).await
            .map_err(|e| ForgeError::AuthenticationFailed { message: e.to_string() })?
            .ok_or_else(|| ForgeError::AuthenticationFailed {
                message: format!("No Codeberg token found for org: {}", self.org),
            })?;

        let mut headers = header::HeaderMap::new();
        // Gitea/Forgejo uses "token" instead of "Bearer"
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("token {}", token))
                .map_err(|e| ForgeError::AuthenticationFailed { message: e.to_string() })?,
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );

        Ok(headers)
    }

    /// Convert Codeberg API response to our Repo type
    fn to_repo(cb_repo: CodebergRepo) -> Repo {
        Repo {
            name: cb_repo.name,
            description: cb_repo.description,
            visibility: if cb_repo.private {
                Visibility::Private
            } else {
                Visibility::Public
            },
            origin: Forge::Codeberg,
            mirrors: Vec::new(),
            protected: cb_repo.archived,
            aliases: Vec::new(),
        }
    }
}

#[async_trait]
impl ForgePort for CodebergAdapter {
    async fn list_repos(&self, org: &str) -> ForgeResult<Vec<Repo>> {
        // Use appropriate endpoint based on account type
        if self.is_user() {
            return self.list_user_repos(org).await;
        }

        let headers = self.auth_headers().await?;
        let url = format!("{}/orgs/{}/repos?limit=100", self.api_url, org);

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "Codeberg API error {}: {}", status, body
            )));
        }

        let cb_repos: Vec<CodebergRepo> = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(cb_repos.into_iter().map(Self::to_repo).collect())
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
                "Codeberg API error {}: {}", status, body
            )));
        }

        let cb_repo: CodebergRepo = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(Self::to_repo(cb_repo))
    }

    async fn create_repo(&self, _org: &str, repo: &Repo) -> ForgeResult<()> {
        // Use appropriate endpoint based on account type
        if self.is_user() {
            return self.create_user_repo(repo).await;
        }

        let headers = self.auth_headers().await?;
        let url = format!("{}/org/{}/repos", self.api_url, self.org);

        let request = CreateRepoRequest {
            name: repo.name.clone(),
            description: repo.description.clone(),
            private: repo.visibility == Visibility::Private,
            auto_init: Some(false),
        };

        let response = self.client.post(&url)
            .headers(headers.clone())
            .json(&request)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::CONFLICT {
            return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
        }

        if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            let body = response.text().await.unwrap_or_default();
            if body.contains("already exists") || body.contains("conflict") {
                return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
            }
            return Err(ForgeError::ApiError(format!("Codeberg API error: {}", body)));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "Codeberg API error {}: {}", status, body
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
                "Codeberg API error {}: {}", status, body
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
                "Codeberg API error {}: {}", status, body
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
                "Codeberg API error {}: {}", status, body
            )));
        }

        Ok(())
    }

    async fn set_default_branch(&self, org: &str, name: &str, branch: &str) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/repos/{}/{}", self.api_url, org, name);

        let request = SetDefaultBranchRequest {
            default_branch: branch.to_string(),
        };

        let response = self.client.patch(&url)
            .headers(headers)
            .json(&request)
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
                "Codeberg API error {}: {}", status, body
            )));
        }

        Ok(())
    }
}

impl CodebergAdapter {
    /// List repos for the authenticated user (includes private repos)
    async fn list_user_repos(&self, _username: &str) -> ForgeResult<Vec<Repo>> {
        let headers = self.auth_headers().await?;
        // Use /user/repos to get authenticated user's repos including private
        let url = format!("{}/user/repos?limit=100", self.api_url);

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "Codeberg API error {}: {}", status, body
            )));
        }

        let cb_repos: Vec<CodebergRepo> = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(cb_repos.into_iter().map(Self::to_repo).collect())
    }

    /// Create repo under authenticated user (fallback when org doesn't exist)
    async fn create_user_repo(&self, repo: &Repo) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/user/repos", self.api_url);

        let request = CreateRepoRequest {
            name: repo.name.clone(),
            description: repo.description.clone(),
            private: repo.visibility == Visibility::Private,
            auto_init: Some(false),
        };

        let response = self.client.post(&url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::CONFLICT {
            return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
        }

        if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            let body = response.text().await.unwrap_or_default();
            if body.contains("already exists") || body.contains("conflict") {
                return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
            }
            return Err(ForgeError::ApiError(format!("Codeberg API error: {}", body)));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "Codeberg API error {}: {}", status, body
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
        let cb_repo = CodebergRepo {
            name: "test-repo".to_string(),
            description: Some("A test repo".to_string()),
            private: false,
            archived: false,
        };

        let repo = CodebergAdapter::to_repo(cb_repo);
        assert_eq!(repo.name, "test-repo");
        assert_eq!(repo.description, Some("A test repo".to_string()));
        assert_eq!(repo.visibility, Visibility::Public);
        assert_eq!(repo.origin, Forge::Codeberg);
        assert!(!repo.protected);
    }

    #[test]
    fn test_to_repo_private_archived() {
        let cb_repo = CodebergRepo {
            name: "private-repo".to_string(),
            description: None,
            private: true,
            archived: true,
        };

        let repo = CodebergAdapter::to_repo(cb_repo);
        assert_eq!(repo.visibility, Visibility::Private);
        assert!(repo.protected); // archived maps to protected
    }

    #[tokio::test]
    async fn test_auth_headers_missing_token() {
        let auth = Arc::new(MockAuthProvider::without_token());
        let adapter = CodebergAdapter::new(auth, "test-org").unwrap();

        let result = adapter.auth_headers().await;
        assert!(matches!(result, Err(ForgeError::AuthenticationFailed { .. })));
    }

    #[tokio::test]
    async fn test_auth_headers_with_token() {
        let auth = Arc::new(MockAuthProvider::with_token("cb_test123"));
        let adapter = CodebergAdapter::new(auth, "test-org").unwrap();

        let headers = adapter.auth_headers().await.unwrap();
        assert!(headers.contains_key(header::AUTHORIZATION));
        assert!(headers.contains_key(header::ACCEPT));
    }
}
