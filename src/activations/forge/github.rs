use async_trait::async_trait;
use async_stream::stream;
use futures::Stream;
use serde_json::Value;
use std::sync::Arc;

use hub_core::plexus::{
    Activation, ChildRouter, PlexusStream, PlexusError,
};
use hub_macro::hub_methods;

use crate::storage::HyperforgePaths;
use crate::events::{ForgeEvent, ForgeRepoSummary};
use crate::types::Forge;

pub struct GitHubRouter {
    #[allow(dead_code)]
    paths: Arc<HyperforgePaths>,
}

impl GitHubRouter {
    pub fn new(paths: Arc<HyperforgePaths>) -> Self {
        Self { paths }
    }
}

#[hub_methods(
    namespace = "github",
    version = "1.0.0",
    description = "GitHub API",
    crate_path = "hub_core"
)]
impl GitHubRouter {
    /// List repositories for a user
    #[hub_method(
        description = "List repositories for a GitHub user",
        params(
            owner = "GitHub username",
            token = "GitHub API token"
        )
    )]
    pub async fn repos_list(&self, owner: String, token: String) -> impl Stream<Item = ForgeEvent> + Send + 'static {
        stream! {
            yield ForgeEvent::ApiProgress {
                forge: Forge::GitHub,
                operation: "repos_list".into(),
                message: format!("Fetching repos for {}", owner),
            };

            let client = reqwest::Client::new();
            let url = format!("https://api.github.com/users/{}/repos?per_page=100", owner);

            match client
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .header("User-Agent", "hyperforge")
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<Vec<serde_json::Value>>().await {
                            Ok(repos) => {
                                let summaries: Vec<ForgeRepoSummary> = repos
                                    .iter()
                                    .filter_map(|r| {
                                        Some(ForgeRepoSummary {
                                            name: r.get("name")?.as_str()?.to_string(),
                                            description: r.get("description")
                                                .and_then(|d| d.as_str())
                                                .map(|s| s.to_string()),
                                            url: r.get("html_url")?.as_str()?.to_string(),
                                            private: r.get("private")?.as_bool()?,
                                        })
                                    })
                                    .collect();

                                yield ForgeEvent::ReposListed {
                                    forge: Forge::GitHub,
                                    owner,
                                    repos: summaries,
                                };
                            }
                            Err(e) => {
                                yield ForgeEvent::Error {
                                    forge: Forge::GitHub,
                                    operation: "repos_list".into(),
                                    message: e.to_string(),
                                    status_code: None,
                                };
                            }
                        }
                    } else {
                        yield ForgeEvent::Error {
                            forge: Forge::GitHub,
                            operation: "repos_list".into(),
                            message: format!("API returned {}", response.status()),
                            status_code: Some(response.status().as_u16()),
                        };
                    }
                }
                Err(e) => {
                    yield ForgeEvent::Error {
                        forge: Forge::GitHub,
                        operation: "repos_list".into(),
                        message: e.to_string(),
                        status_code: None,
                    };
                }
            }
        }
    }

    /// Check authentication status
    #[hub_method(
        description = "Check GitHub authentication status",
        params(token = "GitHub API token")
    )]
    pub async fn auth_status(&self, token: String) -> impl Stream<Item = ForgeEvent> + Send + 'static {
        stream! {
            let client = reqwest::Client::new();

            match client
                .get("https://api.github.com/user")
                .header("Authorization", format!("Bearer {}", token))
                .header("User-Agent", "hyperforge")
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        match response.json::<serde_json::Value>().await {
                            Ok(user) => {
                                yield ForgeEvent::AuthStatus {
                                    forge: Forge::GitHub,
                                    authenticated: true,
                                    user: user.get("login")
                                        .and_then(|l| l.as_str())
                                        .map(|s| s.to_string()),
                                    scopes: vec![],
                                };
                            }
                            Err(_) => {
                                yield ForgeEvent::AuthStatus {
                                    forge: Forge::GitHub,
                                    authenticated: false,
                                    user: None,
                                    scopes: vec![],
                                };
                            }
                        }
                    } else {
                        yield ForgeEvent::AuthStatus {
                            forge: Forge::GitHub,
                            authenticated: false,
                            user: None,
                            scopes: vec![],
                        };
                    }
                }
                Err(e) => {
                    yield ForgeEvent::Error {
                        forge: Forge::GitHub,
                        operation: "auth_status".into(),
                        message: e.to_string(),
                        status_code: None,
                    };
                }
            }
        }
    }
}

#[async_trait]
impl ChildRouter for GitHubRouter {
    fn router_namespace(&self) -> &str {
        "github"
    }

    async fn router_call(&self, method: &str, params: Value) -> Result<PlexusStream, PlexusError> {
        Activation::call(self, method, params).await
    }

    async fn get_child(&self, _name: &str) -> Option<Box<dyn ChildRouter>> {
        None
    }
}
