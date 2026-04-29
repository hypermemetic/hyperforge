//! GitLab `ForgePort` adapter (V5REPOS-11).
//!
//! GitLab REST v4 API. Host is extracted from the `Remote.url`, so
//! self-hosted GitLab works via per-remote `provider: gitlab` override.

use async_trait::async_trait;
use reqwest::{header, Client, StatusCode};
use serde_json::Value;

use crate::v5::adapters::{
    extract_host, ForgeAuth, ForgeMetadata, ForgePort, ForgePortError, MetadataFields,
    ProviderVisibility,
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
        // V5PARITY-24: try explicit token_ref first; fall back to
        // the org's provider-default.
        let value = auth.resolve_token()?;
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

    // -----------------------------------------------------------------
    // V5PROV-5 lifecycle methods.
    // -----------------------------------------------------------------

    async fn create_repo(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        visibility: ProviderVisibility,
        description: &str,
        auth: &ForgeAuth<'_>,
    ) -> Result<(), ForgePortError> {
        // GitLab supports all three visibility variants.
        let token = Self::token(auth).await?;
        let host = Self::host_for(remote)?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;

        // Resolve namespace_id from the org name via `/groups/<org>`.
        // If missing, create under the authenticated user (no
        // namespace_id).
        let group_url = format!("https://{host}/api/v4/groups/{}", repo_ref.org);
        let group_resp = client
            .get(&group_url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("get {group_url}: {e}")))?;
        let mut namespace_id: Option<i64> = None;
        let gstatus = group_resp.status();
        if gstatus.is_success() {
            let v: Value = group_resp
                .json()
                .await
                .map_err(|e| ForgePortError::network(format!("parse gitlab group: {e}")))?;
            namespace_id = v.get("id").and_then(Value::as_i64);
        } else if !matches!(gstatus, StatusCode::NOT_FOUND) {
            let b = group_resp.text().await.unwrap_or_default();
            return Err(Self::map_status_error(gstatus, &b));
        }

        let mut body = serde_json::Map::new();
        body.insert(
            "name".to_string(),
            Value::String(repo_ref.name.as_str().to_string()),
        );
        body.insert(
            "path".to_string(),
            Value::String(repo_ref.name.as_str().to_string()),
        );
        body.insert(
            "visibility".to_string(),
            Value::String(visibility.as_str().to_string()),
        );
        if !description.is_empty() {
            body.insert(
                "description".to_string(),
                Value::String(description.to_string()),
            );
        }
        if let Some(ns) = namespace_id {
            body.insert(
                "namespace_id".to_string(),
                Value::Number(serde_json::Number::from(ns)),
            );
        }

        let create_url = format!("https://{host}/api/v4/projects");
        let resp = client
            .post(&create_url)
            .headers(headers)
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("post {create_url}: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT {
            let b = resp.text().await.unwrap_or_default();
            if b.contains("has already been taken") || b.contains("already exists") {
                return Err(ForgePortError::conflict(format!(
                    "gitlab project '{}/{}' already exists",
                    repo_ref.org, repo_ref.name
                )));
            }
            return Err(ForgePortError::network(format!("gitlab {status}: {b}")));
        }
        let b = resp.text().await.unwrap_or_default();
        Err(Self::map_status_error(status, &b))
    }

    async fn delete_repo(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        auth: &ForgeAuth<'_>,
    ) -> Result<(), ForgePortError> {
        let token = Self::token(auth).await?;
        let host = Self::host_for(remote)?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let url = Self::project_url(&host, repo_ref);

        let resp = client
            .delete(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("delete {url}: {e}")))?;
        let status = resp.status();
        // GitLab returns 202 Accepted for async deletion; both 2xx and
        // 202 mean the request was honored.
        if status.is_success() || status == StatusCode::ACCEPTED {
            return Ok(());
        }
        let b = resp.text().await.unwrap_or_default();
        Err(Self::map_status_error(status, &b))
    }

    async fn repo_exists(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        auth: &ForgeAuth<'_>,
    ) -> Result<bool, ForgePortError> {
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
        if status.is_success() {
            return Ok(true);
        }
        if status == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        let b = resp.text().await.unwrap_or_default();
        Err(Self::map_status_error(status, &b))
    }

    async fn list_repos(
        &self,
        org: &crate::v5::config::OrgName,
        auth: &ForgeAuth<'_>,
    ) -> Result<Vec<crate::v5::adapters::RemoteRepo>, ForgePortError> {
        // GitLab: /groups/{org}/projects — org might be a group OR a user;
        // try group first, fall back to user.
        let token = Self::token(auth).await?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let host = "gitlab.com";
        let mut items = Vec::new();
        let mut page = 1u32;
        let mut tried_user = false;
        loop {
            let url = if tried_user {
                format!("https://{host}/api/v4/users/{}/projects?page={page}&per_page=50", org.as_str())
            } else {
                format!(
                    "https://{host}/api/v4/groups/{}/projects?page={page}&per_page=50",
                    urlencoding::encode(org.as_str())
                )
            };
            let resp = client
                .get(&url)
                .headers(headers.clone())
                .send()
                .await
                .map_err(|e| ForgePortError::network(format!("get {url}: {e}")))?;
            let status = resp.status();
            if status == StatusCode::NOT_FOUND && page == 1 && !tried_user && items.is_empty() {
                tried_user = true;
                continue;
            }
            if !status.is_success() {
                let b = resp.text().await.unwrap_or_default();
                return Err(Self::map_status_error(status, &b));
            }
            let body: serde_json::Value = resp.json().await
                .map_err(|e| ForgePortError::network(format!("parse: {e}")))?;
            let before = items.len();
            if let Some(arr) = body.as_array() {
                for item in arr {
                    let name = item.get("path").and_then(|v| v.as_str())
                        .or_else(|| item.get("name").and_then(|v| v.as_str()))
                        .unwrap_or("").to_string();
                    if name.is_empty() { continue; }
                    let url = item.get("http_url_to_repo").and_then(|v| v.as_str())
                        .or_else(|| item.get("web_url").and_then(|v| v.as_str()))
                        .unwrap_or("").to_string();
                    items.push(crate::v5::adapters::RemoteRepo {
                        name,
                        url,
                        default_branch: item.get("default_branch").and_then(|v| v.as_str()).map(String::from),
                        description: item.get("description").and_then(|v| v.as_str()).map(String::from),
                        archived: item.get("archived").and_then(|v| v.as_bool()),
                        visibility: item.get("visibility").and_then(|v| v.as_str()).map(String::from),
                    });
                }
            }
            if items.len() - before < 50 { break; }
            page += 1;
            if page > 100 { break; }
        }
        Ok(items)
    }

    async fn rename_repo(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        new_name: &str,
        auth: &ForgeAuth<'_>,
    ) -> Result<(), ForgePortError> {
        let token = Self::token(auth).await?;
        let host = Self::host_for(remote)?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let url = Self::project_url(&host, repo_ref);
        let body = serde_json::json!({"name": new_name, "path": new_name});
        let resp = client
            .put(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("put {url}: {e}")))?;
        let status = resp.status();
        if status.is_success() { return Ok(()); }
        let b = resp.text().await.unwrap_or_default();
        Err(Self::map_status_error(status, &b))
    }
}
