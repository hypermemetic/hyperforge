//! Mock ForgeClient implementation for testing
//!
//! This module provides a configurable mock implementation of the ForgeClient trait,
//! allowing tests to simulate forge API responses without making network calls.

use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::types::{Forge, Visibility};

use super::forge_client::{
    AuthStatus, ForgeClient, ForgeError, ForgeRepo, ForgeResult, RepoCreateConfig,
};

/// Configuration for mock responses
#[derive(Debug, Clone, Default)]
pub struct MockConfig {
    /// Repositories to return from list_repos
    pub repos: Vec<ForgeRepo>,
    /// Error to return instead of normal response (if set)
    pub error: Option<MockError>,
    /// Auth status to return from authenticate
    pub auth_status: Option<AuthStatus>,
}

/// Errors that can be configured for the mock
#[derive(Debug, Clone)]
pub enum MockError {
    AuthenticationFailed(String),
    Forbidden(String),
    RateLimited(std::time::Duration),
    RepoNotFound(String),
    RepoAlreadyExists(String),
    ApiError { status: u16, message: String },
    ServerError { status: u16, message: String },
}

impl From<MockError> for ForgeError {
    fn from(err: MockError) -> Self {
        match err {
            MockError::AuthenticationFailed(msg) => {
                ForgeError::AuthenticationFailed { message: msg }
            }
            MockError::Forbidden(msg) => ForgeError::Forbidden { message: msg },
            MockError::RateLimited(duration) => ForgeError::RateLimited {
                retry_after: duration,
            },
            MockError::RepoNotFound(name) => ForgeError::RepoNotFound { name },
            MockError::RepoAlreadyExists(name) => ForgeError::RepoAlreadyExists { name },
            MockError::ApiError { status, message } => ForgeError::ApiError { status, message },
            MockError::ServerError { status, message } => ForgeError::ServerError { status, message },
        }
    }
}

/// A mock implementation of ForgeClient for testing purposes
///
/// # Example
///
/// ```rust,ignore
/// use hyperforge::bridge::mock_forge_client::{MockForgeClient, MockConfig};
/// use hyperforge::bridge::{ForgeClient, ForgeRepo};
/// use hyperforge::types::{Forge, Visibility};
///
/// #[tokio::test]
/// async fn test_list_repos() {
///     let mock = MockForgeClient::new(Forge::GitHub)
///         .with_repos(vec![
///             ForgeRepo {
///                 name: "test-repo".to_string(),
///                 full_name: "owner/test-repo".to_string(),
///                 description: Some("A test repo".to_string()),
///                 visibility: Visibility::Public,
///                 clone_url: "https://github.com/owner/test-repo.git".to_string(),
///                 ssh_url: "git@github.com:owner/test-repo.git".to_string(),
///             }
///         ]);
///
///     let repos = mock.list_repos("owner", "token").await.unwrap();
///     assert_eq!(repos.len(), 1);
///     assert_eq!(repos[0].name, "test-repo");
/// }
/// ```
pub struct MockForgeClient {
    forge: Forge,
    config: Arc<Mutex<MockConfig>>,
    /// Track calls for verification
    call_log: Arc<Mutex<Vec<MockCall>>>,
}

/// Record of a call made to the mock client
#[derive(Debug, Clone)]
pub enum MockCall {
    Authenticate { token: String },
    ListRepos { owner: String, token: String },
    CreateRepo { name: String, config: RepoCreateConfig, token: String },
    DeleteRepo { owner: String, name: String, token: String },
}

impl MockForgeClient {
    /// Create a new mock client for the specified forge
    pub fn new(forge: Forge) -> Self {
        Self {
            forge,
            config: Arc::new(Mutex::new(MockConfig::default())),
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Configure repositories to return from list_repos
    pub fn with_repos(self, repos: Vec<ForgeRepo>) -> Self {
        self.config.lock().unwrap().repos = repos;
        self
    }

    /// Configure an error to return from all operations
    pub fn with_error(self, error: MockError) -> Self {
        self.config.lock().unwrap().error = Some(error);
        self
    }

    /// Configure auth status to return from authenticate
    pub fn with_auth_status(self, status: AuthStatus) -> Self {
        self.config.lock().unwrap().auth_status = Some(status);
        self
    }

    /// Clear any configured error
    pub fn clear_error(&self) {
        self.config.lock().unwrap().error = None;
    }

    /// Get the call log for verification
    pub fn calls(&self) -> Vec<MockCall> {
        self.call_log.lock().unwrap().clone()
    }

    /// Clear the call log
    pub fn clear_calls(&self) {
        self.call_log.lock().unwrap().clear();
    }

    /// Check if a specific method was called
    pub fn was_called(&self, method: &str) -> bool {
        self.call_log.lock().unwrap().iter().any(|call| match (call, method) {
            (MockCall::Authenticate { .. }, "authenticate") => true,
            (MockCall::ListRepos { .. }, "list_repos") => true,
            (MockCall::CreateRepo { .. }, "create_repo") => true,
            (MockCall::DeleteRepo { .. }, "delete_repo") => true,
            _ => false,
        })
    }

    /// Log a call
    fn log_call(&self, call: MockCall) {
        self.call_log.lock().unwrap().push(call);
    }

    /// Check for configured error
    fn check_error(&self) -> ForgeResult<()> {
        let config = self.config.lock().unwrap();
        if let Some(err) = &config.error {
            return Err(err.clone().into());
        }
        Ok(())
    }
}

#[async_trait]
impl ForgeClient for MockForgeClient {
    fn forge(&self) -> Forge {
        self.forge.clone()
    }

    async fn authenticate(&self, token: &str) -> ForgeResult<AuthStatus> {
        self.log_call(MockCall::Authenticate {
            token: token.to_string(),
        });

        self.check_error()?;

        let config = self.config.lock().unwrap();
        Ok(config.auth_status.clone().unwrap_or_else(|| AuthStatus {
            authenticated: true,
            username: "mock-user".to_string(),
            scopes: vec!["repo".to_string(), "read:org".to_string()],
        }))
    }

    async fn list_repos(&self, owner: &str, token: &str) -> ForgeResult<Vec<ForgeRepo>> {
        self.log_call(MockCall::ListRepos {
            owner: owner.to_string(),
            token: token.to_string(),
        });

        self.check_error()?;

        let config = self.config.lock().unwrap();
        Ok(config.repos.clone())
    }

    async fn create_repo(
        &self,
        name: &str,
        config: &RepoCreateConfig,
        token: &str,
    ) -> ForgeResult<ForgeRepo> {
        self.log_call(MockCall::CreateRepo {
            name: name.to_string(),
            config: config.clone(),
            token: token.to_string(),
        });

        self.check_error()?;

        // Check if repo already exists in configured repos
        {
            let mock_config = self.config.lock().unwrap();
            if mock_config.repos.iter().any(|r| r.name == name) {
                return Err(ForgeError::RepoAlreadyExists {
                    name: name.to_string(),
                });
            }
        }

        // Return a new repo matching the request
        let repo = ForgeRepo {
            name: name.to_string(),
            full_name: format!("mock-owner/{}", name),
            description: config.description.clone(),
            visibility: config.visibility.clone(),
            clone_url: format!("https://mock.example.com/mock-owner/{}.git", name),
            ssh_url: format!("git@mock.example.com:mock-owner/{}.git", name),
        };

        // Add to internal list
        self.config.lock().unwrap().repos.push(repo.clone());

        Ok(repo)
    }

    async fn delete_repo(&self, owner: &str, name: &str, token: &str) -> ForgeResult<()> {
        self.log_call(MockCall::DeleteRepo {
            owner: owner.to_string(),
            name: name.to_string(),
            token: token.to_string(),
        });

        self.check_error()?;

        // Check if repo exists
        let mut config = self.config.lock().unwrap();
        let initial_len = config.repos.len();
        config.repos.retain(|r| r.name != name);

        if config.repos.len() == initial_len {
            return Err(ForgeError::RepoNotFound {
                name: name.to_string(),
            });
        }

        Ok(())
    }
}

/// Builder for creating test ForgeRepo instances
pub struct ForgeRepoBuilder {
    repo: ForgeRepo,
}

impl ForgeRepoBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            repo: ForgeRepo {
                name: name.to_string(),
                full_name: format!("test-owner/{}", name),
                description: None,
                visibility: Visibility::Public,
                clone_url: format!("https://example.com/test-owner/{}.git", name),
                ssh_url: format!("git@example.com:test-owner/{}.git", name),
            },
        }
    }

    pub fn with_owner(mut self, owner: &str) -> Self {
        self.repo.full_name = format!("{}/{}", owner, self.repo.name);
        self.repo.clone_url = format!("https://example.com/{}/{}.git", owner, self.repo.name);
        self.repo.ssh_url = format!("git@example.com:{}/{}.git", owner, self.repo.name);
        self
    }

    pub fn with_description(mut self, desc: &str) -> Self {
        self.repo.description = Some(desc.to_string());
        self
    }

    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.repo.visibility = visibility;
        self
    }

    pub fn private(mut self) -> Self {
        self.repo.visibility = Visibility::Private;
        self
    }

    pub fn build(self) -> ForgeRepo {
        self.repo
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_mock_list_repos() {
        let mock = MockForgeClient::new(Forge::GitHub).with_repos(vec![
            ForgeRepoBuilder::new("repo1").build(),
            ForgeRepoBuilder::new("repo2").private().build(),
        ]);

        let repos = mock.list_repos("owner", "token").await.unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].name, "repo1");
        assert_eq!(repos[1].name, "repo2");
        assert!(mock.was_called("list_repos"));
    }

    #[tokio::test]
    async fn test_mock_with_error() {
        let mock = MockForgeClient::new(Forge::GitHub)
            .with_error(MockError::AuthenticationFailed("Invalid token".to_string()));

        let result = mock.authenticate("bad-token").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ForgeError::AuthenticationFailed { message } => {
                assert_eq!(message, "Invalid token");
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_mock_rate_limited() {
        let mock = MockForgeClient::new(Forge::Codeberg)
            .with_error(MockError::RateLimited(Duration::from_secs(60)));

        let result = mock.list_repos("owner", "token").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ForgeError::RateLimited { retry_after } => {
                assert_eq!(retry_after, Duration::from_secs(60));
            }
            _ => panic!("Expected RateLimited error"),
        }
    }

    #[tokio::test]
    async fn test_mock_create_repo() {
        let mock = MockForgeClient::new(Forge::GitHub);

        let config = RepoCreateConfig {
            description: Some("Test repo".to_string()),
            visibility: Visibility::Private,
            auto_init: false,
        };

        let repo = mock.create_repo("new-repo", &config, "token").await.unwrap();
        assert_eq!(repo.name, "new-repo");
        assert_eq!(repo.description, Some("Test repo".to_string()));
        assert!(matches!(repo.visibility, Visibility::Private));

        // Verify it was added to internal list
        let repos = mock.list_repos("owner", "token").await.unwrap();
        assert_eq!(repos.len(), 1);
    }

    #[tokio::test]
    async fn test_mock_create_existing_repo() {
        let mock = MockForgeClient::new(Forge::GitHub)
            .with_repos(vec![ForgeRepoBuilder::new("existing").build()]);

        let config = RepoCreateConfig::default();
        let result = mock.create_repo("existing", &config, "token").await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ForgeError::RepoAlreadyExists { name } => {
                assert_eq!(name, "existing");
            }
            _ => panic!("Expected RepoAlreadyExists error"),
        }
    }

    #[tokio::test]
    async fn test_mock_delete_repo() {
        let mock = MockForgeClient::new(Forge::GitHub)
            .with_repos(vec![ForgeRepoBuilder::new("to-delete").build()]);

        mock.delete_repo("owner", "to-delete", "token").await.unwrap();

        let repos = mock.list_repos("owner", "token").await.unwrap();
        assert!(repos.is_empty());
    }

    #[tokio::test]
    async fn test_mock_delete_nonexistent() {
        let mock = MockForgeClient::new(Forge::GitHub);

        let result = mock.delete_repo("owner", "nonexistent", "token").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ForgeError::RepoNotFound { name } => {
                assert_eq!(name, "nonexistent");
            }
            _ => panic!("Expected RepoNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_mock_call_tracking() {
        let mock = MockForgeClient::new(Forge::GitHub);

        mock.authenticate("token1").await.unwrap();
        mock.list_repos("owner", "token2").await.unwrap();

        let calls = mock.calls();
        assert_eq!(calls.len(), 2);
        assert!(mock.was_called("authenticate"));
        assert!(mock.was_called("list_repos"));
        assert!(!mock.was_called("create_repo"));

        mock.clear_calls();
        assert!(mock.calls().is_empty());
    }

    #[test]
    fn test_forge_repo_builder() {
        let repo = ForgeRepoBuilder::new("my-repo")
            .with_owner("my-org")
            .with_description("A test repository")
            .private()
            .build();

        assert_eq!(repo.name, "my-repo");
        assert_eq!(repo.full_name, "my-org/my-repo");
        assert_eq!(repo.description, Some("A test repository".to_string()));
        assert!(matches!(repo.visibility, Visibility::Private));
    }
}
