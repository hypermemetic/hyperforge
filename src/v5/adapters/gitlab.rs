//! GitLab `ForgePort` adapter (V5REPOS-11).
//!
//! GitLab REST v4 API. Host is extracted from the `Remote.url`, so
//! self-hosted GitLab works via per-remote `provider: gitlab` override.

use async_trait::async_trait;
use reqwest::{header, Client, StatusCode};
use serde_json::Value;

use crate::v5::adapters::{
    extract_host, ForgeAuth, ForgeMetadata, ForgePort, ForgePortError, MetadataFields,
};
use crate::v5::config::{ProviderKind, Remote, RepoRef};

#[derive(Clone, Default)]
pub struct GitlabAdapter;

impl GitlabAdapter {
    #[must_use]
    pub const fn new() -> Self {
        Self
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

    fn host_for(remote: &Remote) -> Result<String, ForgePortError> {
        extract_host(remote.url.as_str())
            .ok_or_else(|| ForgePortError::network(format!("cannot extract host: {}", remote.url)))
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
            "PRIVATE-TOKEN",
            header::HeaderValue::from_str(token)
                .map_err(|_| ForgePortError::auth("invalid token for header"))?,
        );
        h.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        Ok(h)
    }

    fn project_url(host: &str, repo_ref: &RepoRef) -> String {
        let path = format!("{}/{}", repo_ref.org, repo_ref.name);
        let encoded = urlencoding::encode(&path);
        format!("https://{host}/api/v4/projects/{encoded}")
    }

    fn map_status_error(status: StatusCode, body: &str) -> ForgePortError {
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                ForgePortError::auth(format!("gitlab {status}: {body}"))
            }
            StatusCode::NOT_FOUND => {
                ForgePortError::not_found(format!("gitlab 404: {body}"))
            }
            StatusCode::TOO_MANY_REQUESTS => {
                ForgePortError::rate_limited(format!("gitlab {status}: {body}"))
            }
            _ => ForgePortError::network(format!("gitlab {status}: {body}")),
        }
    }
}

#[async_trait]
impl ForgePort for GitlabAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Gitlab
    }

    async fn read_metadata(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        auth: &ForgeAuth<'_>,
    ) -> Result<ForgeMetadata, ForgePortError> {
        let token = Self::token(auth).await?;
        let host = Self::host_for(remote)?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let url = Self::project_url(&host, repo_ref);

        let resp = client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("get {url}: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let b = resp.text().await.unwrap_or_default();
            return Err(Self::map_status_error(status, &b));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ForgePortError::network(format!("parse gitlab body: {e}")))?;

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
        let visibility = v
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("private")
            .to_string();

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
        let token = Self::token(auth).await?;
        let host = Self::host_for(remote)?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let url = Self::project_url(&host, repo_ref);

        let mut body = serde_json::Map::new();
        for (k, v) in fields {
            body.insert(k.as_str().to_string(), v.clone());
        }

        let resp = client
            .put(&url)
            .headers(headers)
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("put {url}: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let b = resp.text().await.unwrap_or_default();
            return Err(Self::map_status_error(status, &b));
        }
        Ok(fields.clone())
    }
}
