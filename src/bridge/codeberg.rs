//! Codeberg API client implementation
//!
//! Implements the `ForgeClient` trait for Codeberg using the Gitea-compatible REST API.
//! See: https://codeberg.org/api/swagger

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::types::{Forge, Visibility};

use super::forge_client::{AuthStatus, ForgeClient, ForgeError, ForgeRepo, ForgeResult, RepoCreateConfig};

/// Codeberg API client (uses Gitea-compatible API)
pub struct CodebergClient {
    client: reqwest::Client,
    base_url: String,
}

impl CodebergClient {
    /// Create a new Codeberg client with default configuration
    pub fn new() -> Self {
        Self::with_base_url("https://codeberg.org/api/v1")
    }

    /// Create a new Codeberg client with a custom base URL
    /// (useful for self-hosted Gitea instances or testing)
    pub fn with_base_url(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_url: base_url.to_string(),
        }
    }

    /// Build headers for Codeberg/Gitea API requests
    fn build_headers(&self, token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        // Codeberg/Gitea uses "token" prefix instead of "Bearer"
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("token {}", token))
                .expect("Invalid token format"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("hyperforge"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers
    }

    /// Handle API response, converting HTTP errors to ForgeError
    async fn handle_response<T: for<'de> Deserialize<'de>>(
        &self,
        response: reqwest::Response,
    ) -> ForgeResult<T> {
        let status = response.status();

        if status.is_success() {
            return response.json().await.map_err(ForgeError::from);
        }

        // Handle specific error codes
        match status.as_u16() {
            401 => Err(ForgeError::AuthenticationFailed {
                message: "Invalid or expired token".to_string(),
            }),
            403 => {
                // Check for rate limiting
                if let Some(retry_after) = response.headers().get("retry-after") {
                    if let Ok(seconds) = retry_after.to_str().unwrap_or("60").parse::<u64>() {
                        return Err(ForgeError::RateLimited {
                            retry_after: Duration::from_secs(seconds),
                        });
                    }
                }
                let body = response.text().await.unwrap_or_default();
                Err(ForgeError::Forbidden { message: body })
            }
            404 => {
                let body = response.text().await.unwrap_or_default();
                Err(ForgeError::ApiError {
                    status: 404,
                    message: body,
                })
            }
            429 => {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(60);
                Err(ForgeError::RateLimited {
                    retry_after: Duration::from_secs(retry_after),
                })
            }
            500..=599 => {
                let body = response.text().await.unwrap_or_default();
                Err(ForgeError::ServerError {
                    status: status.as_u16(),
                    message: body,
                })
            }
            _ => {
                let body = response.text().await.unwrap_or_default();
                Err(ForgeError::ApiError {
                    status: status.as_u16(),
                    message: body,
                })
            }
        }
    }
}

impl Default for CodebergClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Gitea user response
#[derive(Debug, Deserialize)]
struct GiteaUser {
    login: String,
}

/// Gitea repository response
#[derive(Debug, Deserialize)]
struct GiteaRepo {
    name: String,
    full_name: String,
    description: Option<String>,
    private: bool,
    clone_url: String,
    ssh_url: String,
}

impl From<GiteaRepo> for ForgeRepo {
    fn from(repo: GiteaRepo) -> Self {
        ForgeRepo {
            name: repo.name,
            full_name: repo.full_name,
            description: repo.description,
            visibility: if repo.private {
                Visibility::Private
            } else {
                Visibility::Public
            },
            clone_url: repo.clone_url,
            ssh_url: repo.ssh_url,
        }
    }
}

/// Gitea create repository request
#[derive(Debug, Serialize)]
struct CreateRepoRequest {
    name: String,
    description: Option<String>,
    private: bool,
    auto_init: bool,
}

#[async_trait]
impl ForgeClient for CodebergClient {
    fn forge(&self) -> Forge {
        Forge::Codeberg
    }

    async fn authenticate(&self, token: &str) -> ForgeResult<AuthStatus> {
        let url = format!("{}/user", self.base_url);
        let headers = self.build_headers(token);

        let response = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await?;

        let user: GiteaUser = self.handle_response(response).await?;

        Ok(AuthStatus {
            authenticated: true,
            username: user.login,
            // Gitea doesn't expose scopes in the same way as GitHub
            scopes: vec![],
        })
    }

    async fn list_repos(&self, owner: &str, token: &str) -> ForgeResult<Vec<ForgeRepo>> {
        let headers = self.build_headers(token);
        let mut all_repos = Vec::new();
        let mut page = 1;

        loop {
            // Gitea API uses /users/{owner}/repos
            let url = format!(
                "{}/users/{}/repos?limit=50&page={}",
                self.base_url, owner, page
            );

            let response = self.client.get(&url).headers(headers.clone()).send().await?;

            let repos: Vec<GiteaRepo> = self.handle_response(response).await?;

            if repos.is_empty() {
                break;
            }

            all_repos.extend(repos.into_iter().map(ForgeRepo::from));
            page += 1;

            // Safety limit to prevent infinite loops
            if page > 100 {
                break;
            }
        }

        Ok(all_repos)
    }

    async fn create_repo(
        &self,
        name: &str,
        config: &RepoCreateConfig,
        token: &str,
    ) -> ForgeResult<ForgeRepo> {
        // Gitea uses /user/repos to create repos for the authenticated user
        let url = format!("{}/user/repos", self.base_url);
        let headers = self.build_headers(token);

        let request = CreateRepoRequest {
            name: name.to_string(),
            description: config.description.clone(),
            private: matches!(config.visibility, Visibility::Private),
            auto_init: config.auto_init,
        };

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .json(&request)
            .send()
            .await?;

        // Check for 409 Conflict which indicates repo already exists
        if response.status().as_u16() == 409 {
            return Err(ForgeError::RepoAlreadyExists {
                name: name.to_string(),
            });
        }

        let repo: GiteaRepo = self.handle_response(response).await?;
        Ok(repo.into())
    }

    async fn delete_repo(&self, owner: &str, name: &str, token: &str) -> ForgeResult<()> {
        let url = format!("{}/repos/{}/{}", self.base_url, owner, name);
        let headers = self.build_headers(token);

        let response = self.client.delete(&url).headers(headers).send().await?;

        if response.status().as_u16() == 404 {
            return Err(ForgeError::RepoNotFound {
                name: format!("{}/{}", owner, name),
            });
        }

        // Gitea returns 204 No Content on success
        if response.status().is_success() {
            return Ok(());
        }

        // Handle other errors
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();

        match status {
            403 => Err(ForgeError::Forbidden { message: body }),
            _ => Err(ForgeError::ApiError {
                status,
                message: body,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codeberg_client_default() {
        let client = CodebergClient::new();
        assert_eq!(client.base_url, "https://codeberg.org/api/v1");
        assert_eq!(client.forge(), Forge::Codeberg);
    }

    #[test]
    fn test_codeberg_client_custom_url() {
        let client = CodebergClient::with_base_url("https://gitea.example.com/api/v1");
        assert_eq!(client.base_url, "https://gitea.example.com/api/v1");
    }

    #[test]
    fn test_gitea_repo_conversion() {
        let gitea_repo = GiteaRepo {
            name: "test-repo".to_string(),
            full_name: "user/test-repo".to_string(),
            description: Some("A test repository".to_string()),
            private: false,
            clone_url: "https://codeberg.org/user/test-repo.git".to_string(),
            ssh_url: "git@codeberg.org:user/test-repo.git".to_string(),
        };

        let forge_repo: ForgeRepo = gitea_repo.into();

        assert_eq!(forge_repo.name, "test-repo");
        assert_eq!(forge_repo.full_name, "user/test-repo");
        assert_eq!(forge_repo.description, Some("A test repository".to_string()));
        assert!(matches!(forge_repo.visibility, Visibility::Public));
        assert_eq!(forge_repo.clone_url, "https://codeberg.org/user/test-repo.git");
        assert_eq!(forge_repo.ssh_url, "git@codeberg.org:user/test-repo.git");
    }
}
