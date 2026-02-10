//! GitLab adapter implementing ForgePort trait
//!
//! Uses the GitLab REST API v4 to manage repositories.

use async_trait::async_trait;
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::AuthProvider;
use crate::types::{Forge, Repo, Visibility};
use super::{ForgeError, ForgePort, ForgeResult};

/// GitLab API base URL
const GITLAB_API_URL: &str = "https://gitlab.com/api/v4";

/// GitLab project response from API
#[derive(Debug, Deserialize)]
struct GitLabProject {
    name: String,
    description: Option<String>,
    visibility: String, // "public", "internal", "private"
    #[serde(default)]
    archived: bool,
}

/// Request body for creating a project
#[derive(Debug, Serialize)]
struct CreateProjectRequest {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    visibility: String,
    namespace_id: Option<i64>,
}

/// Request body for updating a project
#[derive(Debug, Serialize)]
struct UpdateProjectRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<String>,
}

/// Request body for renaming a project
#[derive(Debug, Serialize)]
struct RenameProjectRequest {
    name: String,
    path: String,
}

/// GitLab group response
#[derive(Debug, Deserialize)]
struct GitLabGroup {
    id: i64,
}

/// GitLab adapter for ForgePort trait
pub struct GitLabAdapter {
    client: Client,
    auth: Arc<dyn AuthProvider>,
    api_url: String,
    org: String,
}

impl GitLabAdapter {
    /// Create a new GitLabAdapter with the given auth provider
    pub fn new(auth: Arc<dyn AuthProvider>, org: impl Into<String>) -> ForgeResult<Self> {
        Self::with_api_url(auth, org, GITLAB_API_URL.to_string())
    }

    /// Create a new GitLabAdapter with a custom API URL (for testing or self-hosted)
    pub fn with_api_url(auth: Arc<dyn AuthProvider>, org: impl Into<String>, api_url: String) -> ForgeResult<Self> {
        let client = Client::builder()
            .user_agent("hyperforge/2.0")
            .build()
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        Ok(Self { client, auth, api_url, org: org.into() })
    }

    /// Get authorization headers with token from auth provider
    async fn auth_headers(&self) -> ForgeResult<header::HeaderMap> {
        // Construct secret path: gitlab/{org}/token
        let secret_path = format!("gitlab/{}/token", self.org);
        let token = self.auth.get_secret(&secret_path).await
            .map_err(|e| ForgeError::AuthenticationFailed { message: e.to_string() })?
            .ok_or_else(|| ForgeError::AuthenticationFailed {
                message: format!("No GitLab token found for org: {}", self.org),
            })?;

        let mut headers = header::HeaderMap::new();
        // GitLab supports both PRIVATE-TOKEN header and Bearer token
        headers.insert(
            "PRIVATE-TOKEN",
            header::HeaderValue::from_str(&token)
                .map_err(|e| ForgeError::AuthenticationFailed { message: e.to_string() })?,
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );

        Ok(headers)
    }

    /// Get group ID by name
    async fn get_group_id(&self, group_name: &str) -> ForgeResult<i64> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/groups/{}", self.api_url, group_name);

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ForgeError::ApiError(format!("Group '{}' not found", group_name)));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitLab API error {}: {}", status, body
            )));
        }

        let group: GitLabGroup = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(group.id)
    }

    /// Convert GitLab API response to our Repo type
    fn to_repo(gl_project: GitLabProject) -> Repo {
        let visibility = match gl_project.visibility.as_str() {
            "public" => Visibility::Public,
            "internal" => Visibility::Public, // Treat internal as public for our purposes
            _ => Visibility::Private,
        };

        Repo {
            name: gl_project.name,
            description: gl_project.description,
            visibility,
            origin: Forge::GitLab,
            mirrors: Vec::new(),
            protected: gl_project.archived,
        }
    }

    /// Convert our Visibility to GitLab visibility string
    fn to_gitlab_visibility(vis: &Visibility) -> String {
        match vis {
            Visibility::Public => "public".to_string(),
            Visibility::Private => "private".to_string(),
        }
    }
}

#[async_trait]
impl ForgePort for GitLabAdapter {
    async fn list_repos(&self, org: &str) -> ForgeResult<Vec<Repo>> {
        let headers = self.auth_headers().await?;
        // Try as group first
        let url = format!("{}/groups/{}/projects?per_page=100", self.api_url, org);

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            // Try user repos if group not found
            return self.list_user_repos(org).await;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitLab API error {}: {}", status, body
            )));
        }

        let gl_projects: Vec<GitLabProject> = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(gl_projects.into_iter().map(Self::to_repo).collect())
    }

    async fn get_repo(&self, org: &str, name: &str) -> ForgeResult<Repo> {
        let headers = self.auth_headers().await?;
        // GitLab uses URL-encoded "namespace/project" as project ID
        let project_path = format!("{}/{}", org, name);
        let encoded_path = urlencoding::encode(&project_path);
        let url = format!("{}/projects/{}", self.api_url, encoded_path);

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
                "GitLab API error {}: {}", status, body
            )));
        }

        let gl_project: GitLabProject = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(Self::to_repo(gl_project))
    }

    async fn create_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;

        // Get group/namespace ID
        let namespace_id = match self.get_group_id(org).await {
            Ok(id) => Some(id),
            Err(_) => None, // Will create under user if group not found
        };

        let url = format!("{}/projects", self.api_url);

        let request = CreateProjectRequest {
            name: repo.name.clone(),
            description: repo.description.clone(),
            visibility: Self::to_gitlab_visibility(&repo.visibility),
            namespace_id,
        };

        let response = self.client.post(&url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            let body = response.text().await.unwrap_or_default();
            if body.contains("has already been taken") || body.contains("already exists") {
                return Err(ForgeError::RepoAlreadyExists { name: repo.name.clone() });
            }
            return Err(ForgeError::ApiError(format!("GitLab API error: {}", body)));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitLab API error {}: {}", status, body
            )));
        }

        Ok(())
    }

    async fn update_repo(&self, org: &str, repo: &Repo) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let project_path = format!("{}/{}", org, repo.name);
        let encoded_path = urlencoding::encode(&project_path);
        let url = format!("{}/projects/{}", self.api_url, encoded_path);

        let request = UpdateProjectRequest {
            description: repo.description.clone(),
            visibility: Some(Self::to_gitlab_visibility(&repo.visibility)),
        };

        let response = self.client.put(&url)
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
                "GitLab API error {}: {}", status, body
            )));
        }

        Ok(())
    }

    async fn delete_repo(&self, org: &str, name: &str) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let project_path = format!("{}/{}", org, name);
        let encoded_path = urlencoding::encode(&project_path);
        let url = format!("{}/projects/{}", self.api_url, encoded_path);

        let response = self.client.delete(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            // Already deleted, treat as success
            return Ok(());
        }

        // GitLab returns 202 Accepted for async deletion
        if response.status() == reqwest::StatusCode::ACCEPTED || response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(ForgeError::ApiError(format!(
            "GitLab API error {}: {}", status, body
        )))
    }

    async fn rename_repo(&self, org: &str, old_name: &str, new_name: &str) -> ForgeResult<()> {
        let headers = self.auth_headers().await?;
        let project_path = format!("{}/{}", org, old_name);
        let encoded_path = urlencoding::encode(&project_path);
        let url = format!("{}/projects/{}", self.api_url, encoded_path);

        // GitLab requires both name and path to be updated for a rename
        let request = RenameProjectRequest {
            name: new_name.to_string(),
            path: new_name.to_string(),
        };

        let response = self.client.put(&url)
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
                "GitLab API error {}: {}", status, body
            )));
        }

        Ok(())
    }
}

impl GitLabAdapter {
    /// List repos for a user (fallback when group doesn't exist)
    async fn list_user_repos(&self, username: &str) -> ForgeResult<Vec<Repo>> {
        let headers = self.auth_headers().await?;
        let url = format!("{}/users/{}/projects?per_page=100", self.api_url, username);

        let response = self.client.get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError(format!(
                "GitLab API error {}: {}", status, body
            )));
        }

        let gl_projects: Vec<GitLabProject> = response.json().await
            .map_err(|e| ForgeError::ApiError(format!("Failed to parse response: {}", e)))?;

        Ok(gl_projects.into_iter().map(Self::to_repo).collect())
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
        let gl_project = GitLabProject {
            name: "test-repo".to_string(),
            description: Some("A test repo".to_string()),
            visibility: "public".to_string(),
            archived: false,
        };

        let repo = GitLabAdapter::to_repo(gl_project);
        assert_eq!(repo.name, "test-repo");
        assert_eq!(repo.description, Some("A test repo".to_string()));
        assert_eq!(repo.visibility, Visibility::Public);
        assert_eq!(repo.origin, Forge::GitLab);
        assert!(!repo.protected);
    }

    #[test]
    fn test_to_repo_private_archived() {
        let gl_project = GitLabProject {
            name: "private-repo".to_string(),
            description: None,
            visibility: "private".to_string(),
            archived: true,
        };

        let repo = GitLabAdapter::to_repo(gl_project);
        assert_eq!(repo.visibility, Visibility::Private);
        assert!(repo.protected); // archived maps to protected
    }

    #[test]
    fn test_to_repo_internal() {
        let gl_project = GitLabProject {
            name: "internal-repo".to_string(),
            description: None,
            visibility: "internal".to_string(),
            archived: false,
        };

        let repo = GitLabAdapter::to_repo(gl_project);
        assert_eq!(repo.visibility, Visibility::Public); // internal treated as public
    }

    /*
    #[tokio::test]
    async fn test_auth_headers_missing_token() {
        let auth = Arc::new(MockAuthProvider::without_token());
        let adapter = GitLabAdapter::new(auth).unwrap();

        let result = adapter.auth_headers().await;
        assert!(matches!(result, Err(ForgeError::AuthenticationFailed { .. })));
    }
    */

    /*
    #[tokio::test]
    async fn test_auth_headers_with_token() {
        let auth = Arc::new(MockAuthProvider::with_token("glpat-test123"));
        let adapter = GitLabAdapter::new(auth).unwrap();

        let headers = adapter.auth_headers().await.unwrap();
        assert!(headers.contains_key("PRIVATE-TOKEN"));
        assert!(headers.contains_key(header::ACCEPT));
    }
    */
}
