//! GitHub API client implementation
//!
//! Implements the `ForgeClient` trait for GitHub using the GitHub REST API.
//! See: https://docs.github.com/en/rest

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::types::{Forge, Visibility};

use super::forge_client::{AuthStatus, ForgeClient, ForgeError, ForgeRepo, ForgeResult, RepoCreateConfig};

/// GitHub API client
pub struct GitHubClient {
    client: reqwest::Client,
    base_url: String,
}

impl GitHubClient {
    /// Create a new GitHub client with default configuration
    pub fn new() -> Self {
        Self::with_base_url("https://api.github.com")
    }

    /// Create a new GitHub client with a custom base URL
    /// (useful for GitHub Enterprise or testing)
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

    /// Build headers for GitHub API requests
    fn build_headers(&self, token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token))
                .expect("Invalid token format"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("hyperforge"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );
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
                // Check X-RateLimit-Remaining
                if let Some(remaining) = response.headers().get("x-ratelimit-remaining") {
                    if remaining.to_str().unwrap_or("1") == "0" {
                        // Rate limited - use reset time if available
                        let retry_secs = response
                            .headers()
                            .get("x-ratelimit-reset")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .map(|reset| {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                reset.saturating_sub(now)
                            })
                            .unwrap_or(60);
                        return Err(ForgeError::RateLimited {
                            retry_after: Duration::from_secs(retry_secs),
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

impl Default for GitHubClient {
    fn default() -> Self {
        Self::new()
    }
}

/// GitHub user response
#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: String,
}

/// GitHub repository response
#[derive(Debug, Deserialize)]
struct GitHubRepo {
    name: String,
    full_name: String,
    description: Option<String>,
    private: bool,
    clone_url: String,
    ssh_url: String,
}

impl From<GitHubRepo> for ForgeRepo {
    fn from(repo: GitHubRepo) -> Self {
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

/// GitHub create repository request
#[derive(Debug, Serialize)]
struct CreateRepoRequest {
    name: String,
    description: Option<String>,
    private: bool,
    auto_init: bool,
}

#[async_trait]
impl ForgeClient for GitHubClient {
    fn forge(&self) -> Forge {
        Forge::GitHub
    }

    async fn authenticate(&self, token: &str) -> ForgeResult<AuthStatus> {
        let url = format!("{}/user", self.base_url);
        let headers = self.build_headers(token);

        let response = self
            .client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await?;

        // Extract scopes from response headers before consuming body
        let scopes: Vec<String> = response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(", ").map(|s| s.to_string()).collect())
            .unwrap_or_default();

        let user: GitHubUser = self.handle_response(response).await?;

        Ok(AuthStatus {
            authenticated: true,
            username: user.login,
            scopes,
        })
    }

    async fn list_repos(&self, owner: &str, token: &str) -> ForgeResult<Vec<ForgeRepo>> {
        let headers = self.build_headers(token);
        let mut all_repos = Vec::new();
        let mut page = 1;

        loop {
            let url = format!(
                "{}/users/{}/repos?per_page=100&page={}",
                self.base_url, owner, page
            );

            let response = self.client.get(&url).headers(headers.clone()).send().await?;

            let repos: Vec<GitHubRepo> = self.handle_response(response).await?;

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

        // Check for 422 which indicates repo already exists
        if response.status().as_u16() == 422 {
            return Err(ForgeError::RepoAlreadyExists {
                name: name.to_string(),
            });
        }

        let repo: GitHubRepo = self.handle_response(response).await?;
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

        // GitHub returns 204 No Content on success
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
    fn test_github_client_default() {
        let client = GitHubClient::new();
        assert_eq!(client.base_url, "https://api.github.com");
        assert_eq!(client.forge(), Forge::GitHub);
    }

    #[test]
    fn test_github_client_custom_url() {
        let client = GitHubClient::with_base_url("https://github.example.com/api/v3");
        assert_eq!(client.base_url, "https://github.example.com/api/v3");
    }

    #[test]
    fn test_github_repo_conversion() {
        let github_repo = GitHubRepo {
            name: "test-repo".to_string(),
            full_name: "user/test-repo".to_string(),
            description: Some("A test repository".to_string()),
            private: true,
            clone_url: "https://github.com/user/test-repo.git".to_string(),
            ssh_url: "git@github.com:user/test-repo.git".to_string(),
        };

        let forge_repo: ForgeRepo = github_repo.into();

        assert_eq!(forge_repo.name, "test-repo");
        assert_eq!(forge_repo.full_name, "user/test-repo");
        assert_eq!(forge_repo.description, Some("A test repository".to_string()));
        assert!(matches!(forge_repo.visibility, Visibility::Private));
        assert_eq!(forge_repo.clone_url, "https://github.com/user/test-repo.git");
        assert_eq!(forge_repo.ssh_url, "git@github.com:user/test-repo.git");
    }
}
