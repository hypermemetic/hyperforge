//! GitHub `ForgePort` adapter (V5REPOS-9).
//!
//! Reads/writes the D3 intersection against `api.github.com`.

use async_trait::async_trait;
use reqwest::{header, Client, StatusCode};
use serde_json::Value;

use crate::v5::adapters::{
    extract_host, ForgeAuth, ForgeMetadata, ForgePort, ForgePortError, MetadataFields,
};
use crate::v5::config::{ProviderKind, Remote, RepoRef};

const DEFAULT_HOST: &str = "api.github.com";

/// GitHub adapter. Host-pinned to `api.github.com` for v1; GHES is a
/// future extension.
#[derive(Clone, Default)]
pub struct GithubAdapter {
    api_host: String,
}

impl GithubAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            api_host: DEFAULT_HOST.to_string(),
        }
    }

    async fn token(auth: &ForgeAuth<'_>) -> Result<String, ForgePortError> {
        let token_ref = auth
            .token_ref
            .ok_or_else(|| ForgePortError::auth("no token credential on org"))?;
        let parsed = crate::v5::secrets::SecretRef::parse(token_ref)
            .map_err(|e| ForgePortError::auth(format!("invalid secret ref: {e}")))?;
        let value = auth
            .resolver
            .resolve(&parsed)
            .map_err(|e| ForgePortError::auth(format!("resolve {token_ref}: {e}")))?;
        if value.trim().is_empty() {
            return Err(ForgePortError::auth("token is empty"));
        }
        Ok(value)
    }

    fn api_url(&self, repo_ref: &RepoRef) -> String {
        format!(
            "https://{}/repos/{}/{}",
            self.api_host, repo_ref.org, repo_ref.name
        )
    }

    fn build_client() -> Result<Client, ForgePortError> {
        Client::builder()
            .user_agent("hyperforge-v5")
            .build()
            .map_err(|e| ForgePortError::network(format!("client build: {e}")))
    }

    fn auth_headers(token: &str) -> Result<header::HeaderMap, ForgePortError> {
        let mut h = header::HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| ForgePortError::auth("invalid token for header"))?,
        );
        h.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        h.insert(
            "X-GitHub-Api-Version",
            header::HeaderValue::from_static("2022-11-28"),
        );
        Ok(h)
    }

    fn map_status_error(status: StatusCode, body: &str) -> ForgePortError {
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                // GitHub returns 403 for rate limit with specific header;
                // we detect via body tokens.
                if body.contains("rate limit") || body.contains("API rate limit exceeded") {
                    ForgePortError::rate_limited(format!("github {status}: {body}"))
                } else {
                    ForgePortError::auth(format!("github {status}: {body}"))
                }
            }
            StatusCode::NOT_FOUND => {
                ForgePortError::not_found(format!("github 404: {body}"))
            }
            StatusCode::TOO_MANY_REQUESTS => {
                ForgePortError::rate_limited(format!("github {status}: {body}"))
            }
            _ => ForgePortError::network(format!("github {status}: {body}")),
        }
    }
}

#[async_trait]
impl ForgePort for GithubAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Github
    }

    async fn read_metadata(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        auth: &ForgeAuth<'_>,
    ) -> Result<ForgeMetadata, ForgePortError> {
        let _ = extract_host(remote.url.as_str()); // validate
        let token = Self::token(auth).await?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let url = self.api_url(repo_ref);

        let resp = client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("get {url}: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::map_status_error(status, &body));
        }

        let v: Value = resp
            .json()
            .await
            .map_err(|e| ForgePortError::network(format!("parse github body: {e}")))?;

        let default_branch = v
            .get("default_branch")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let description = v
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let archived = v.get("archived").and_then(Value::as_bool).unwrap_or(false);

        // GitHub tri-state visibility: prefer `visibility` when present,
        // else fall back to the boolean `private`.
        let visibility = if let Some(vis) = v.get("visibility").and_then(Value::as_str) {
            vis.to_string()
        } else if v.get("private").and_then(Value::as_bool).unwrap_or(false) {
            "private".to_string()
        } else {
            "public".to_string()
        };

        Ok(ForgeMetadata {
            default_branch,
            description,
            archived,
            visibility,
        })
    }

    async fn write_metadata(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        fields: &MetadataFields,
        auth: &ForgeAuth<'_>,
    ) -> Result<MetadataFields, ForgePortError> {
        let _ = extract_host(remote.url.as_str());
        let token = Self::token(auth).await?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let url = self.api_url(repo_ref);

        let mut body = serde_json::Map::new();
        for (k, v) in fields {
            body.insert(k.as_str().to_string(), v.clone());
        }
        let resp = client
            .patch(&url)
            .headers(headers)
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("patch {url}: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let b = resp.text().await.unwrap_or_default();
            return Err(Self::map_status_error(status, &b));
        }
        // Echo the applied fields back.
        Ok(fields.clone())
    }
}
