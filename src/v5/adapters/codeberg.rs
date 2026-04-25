//! Codeberg `ForgePort` adapter (V5REPOS-10).
//!
//! Gitea REST API against `codeberg.org`.

use async_trait::async_trait;
use reqwest::{header, Client, StatusCode};
use serde_json::Value;

use crate::v5::adapters::{
    extract_host, ForgeAuth, ForgeMetadata, ForgePort, ForgePortError, MetadataFields,
    ProviderVisibility,
};
use crate::v5::config::{ProviderKind, Remote, RepoRef};

const DEFAULT_HOST: &str = "codeberg.org";

#[derive(Clone, Default)]
pub struct CodebergAdapter {
    host: String,
}

impl CodebergAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
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
            "https://{}/api/v1/repos/{}/{}",
            self.host, repo_ref.org, repo_ref.name
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
            header::HeaderValue::from_str(&format!("token {token}"))
                .map_err(|_| ForgePortError::auth("invalid token for header"))?,
        );
        h.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        Ok(h)
    }

    fn map_status_error(status: StatusCode, body: &str) -> ForgePortError {
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                ForgePortError::auth(format!("codeberg {status}: {body}"))
            }
            StatusCode::NOT_FOUND => {
                ForgePortError::not_found(format!("codeberg 404: {body}"))
            }
            StatusCode::TOO_MANY_REQUESTS => {
                ForgePortError::rate_limited(format!("codeberg {status}: {body}"))
            }
            _ => ForgePortError::network(format!("codeberg {status}: {body}")),
        }
    }
}

#[async_trait]
impl ForgePort for CodebergAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Codeberg
    }

    async fn read_metadata(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        auth: &ForgeAuth<'_>,
    ) -> Result<ForgeMetadata, ForgePortError> {
        let _ = extract_host(remote.url.as_str());
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
            let b = resp.text().await.unwrap_or_default();
            return Err(Self::map_status_error(status, &b));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ForgePortError::network(format!("parse codeberg body: {e}")))?;

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
        let private = v.get("private").and_then(Value::as_bool).unwrap_or(false);
        let visibility = if private { "private" } else { "public" }.to_string();

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

        // Translate shared fields into Gitea shape: visibility →
        // `private: bool`. Gitea's patch endpoint takes `description`,
        // `archived`, `default_branch`, `private`.
        let mut body = serde_json::Map::new();
        for (k, v) in fields {
            match k {
                crate::v5::adapters::DriftFieldKind::Visibility => {
                    let s = v.as_str().unwrap_or("public");
                    body.insert(
                        "private".to_string(),
                        Value::Bool(matches!(s, "private")),
                    );
                }
                other => {
                    body.insert(other.as_str().to_string(), v.clone());
                }
            }
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
        Ok(fields.clone())
    }

    // -----------------------------------------------------------------
    // V5PROV-4 lifecycle methods.
    // -----------------------------------------------------------------

    async fn create_repo(
        &self,
        remote: &Remote,
        repo_ref: &RepoRef,
        visibility: ProviderVisibility,
        description: &str,
        auth: &ForgeAuth<'_>,
    ) -> Result<(), ForgePortError> {
        // Gitea v1 has no `internal` visibility in our portable surface.
        if matches!(visibility, ProviderVisibility::Internal) {
            return Err(ForgePortError::unsupported_visibility(
                "codeberg.org (Gitea) does not support visibility 'internal'",
            ));
        }
        let _ = extract_host(remote.url.as_str());
        let token = Self::token(auth).await?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;

        let private = matches!(visibility, ProviderVisibility::Private);
        let mut body = serde_json::Map::new();
        body.insert(
            "name".to_string(),
            Value::String(repo_ref.name.as_str().to_string()),
        );
        body.insert("private".to_string(), Value::Bool(private));
        if !description.is_empty() {
            body.insert(
                "description".to_string(),
                Value::String(description.to_string()),
            );
        }

        // Org endpoint first; fall back to `/user/repos` on 404.
        let org_url = format!(
            "https://{}/api/v1/orgs/{}/repos",
            self.host, repo_ref.org
        );
        let resp = client
            .post(&org_url)
            .headers(headers.clone())
            .json(&Value::Object(body.clone()))
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("post {org_url}: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if status == StatusCode::CONFLICT {
            return Err(ForgePortError::conflict(format!(
                "codeberg repo '{}/{}' already exists",
                repo_ref.org, repo_ref.name
            )));
        }
        if status == StatusCode::UNPROCESSABLE_ENTITY {
            let b = resp.text().await.unwrap_or_default();
            if b.contains("already exists") || b.contains("taken") {
                return Err(ForgePortError::conflict(format!(
                    "codeberg repo '{}/{}' already exists",
                    repo_ref.org, repo_ref.name
                )));
            }
            return Err(ForgePortError::network(format!("codeberg 422: {b}")));
        }
        if status == StatusCode::NOT_FOUND {
            let user_url = format!("https://{}/api/v1/user/repos", self.host);
            let resp2 = client
                .post(&user_url)
                .headers(headers)
                .json(&Value::Object(body))
                .send()
                .await
                .map_err(|e| ForgePortError::network(format!("post {user_url}: {e}")))?;
            let s2 = resp2.status();
            if s2.is_success() {
                return Ok(());
            }
            if s2 == StatusCode::CONFLICT {
                return Err(ForgePortError::conflict(format!(
                    "codeberg repo '{}/{}' already exists",
                    repo_ref.org, repo_ref.name
                )));
            }
            let b = resp2.text().await.unwrap_or_default();
            return Err(Self::map_status_error(s2, &b));
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
        let _ = extract_host(remote.url.as_str());
        let token = Self::token(auth).await?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let url = self.api_url(repo_ref);

        let resp = client
            .delete(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("delete {url}: {e}")))?;
        let status = resp.status();
        if status.is_success() || status == StatusCode::NO_CONTENT {
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
        let _ = extract_host(remote.url.as_str());
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
        // Gitea: /orgs/{org}/repos with pagination (?page=N&limit=50)
        let token = Self::token(auth).await?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let mut items = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!(
                "https://codeberg.org/api/v1/orgs/{}/repos?page={page}&limit=50",
                org.as_str()
            );
            let resp = client
                .get(&url)
                .headers(headers.clone())
                .send()
                .await
                .map_err(|e| ForgePortError::network(format!("get {url}: {e}")))?;
            let status = resp.status();
            if status == StatusCode::NOT_FOUND && page == 1 && items.is_empty() {
                // try user endpoint
                let alt = format!(
                    "https://codeberg.org/api/v1/users/{}/repos?page=1&limit=50",
                    org.as_str()
                );
                let r2 = client
                    .get(&alt)
                    .headers(headers.clone())
                    .send()
                    .await
                    .map_err(|e| ForgePortError::network(format!("get {alt}: {e}")))?;
                if !r2.status().is_success() {
                    let b = r2.text().await.unwrap_or_default();
                    return Err(Self::map_status_error(status, &b));
                }
                let body: serde_json::Value = r2.json().await
                    .map_err(|e| ForgePortError::network(format!("parse: {e}")))?;
                push_gitea_items(&mut items, &body);
                break;
            }
            if !status.is_success() {
                let b = resp.text().await.unwrap_or_default();
                return Err(Self::map_status_error(status, &b));
            }
            let body: serde_json::Value = resp.json().await
                .map_err(|e| ForgePortError::network(format!("parse: {e}")))?;
            let before = items.len();
            push_gitea_items(&mut items, &body);
            if items.len() - before < 50 {
                break;
            }
            page += 1;
            if page > 100 { break; } // safety cap
        }
        Ok(items)
    }

    async fn rename_repo(
        &self,
        _remote: &Remote,
        repo_ref: &RepoRef,
        new_name: &str,
        auth: &ForgeAuth<'_>,
    ) -> Result<(), ForgePortError> {
        let token = Self::token(auth).await?;
        let client = Self::build_client()?;
        let headers = Self::auth_headers(&token)?;
        let url = self.api_url(repo_ref);
        let body = serde_json::json!({"name": new_name});
        let resp = client
            .patch(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| ForgePortError::network(format!("patch {url}: {e}")))?;
        let status = resp.status();
        if status.is_success() { return Ok(()); }
        let b = resp.text().await.unwrap_or_default();
        Err(Self::map_status_error(status, &b))
    }
}

fn push_gitea_items(items: &mut Vec<crate::v5::adapters::RemoteRepo>, body: &serde_json::Value) {
    if let Some(arr) = body.as_array() {
        for item in arr {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if name.is_empty() { continue; }
            let url = item.get("clone_url").and_then(|v| v.as_str())
                .or_else(|| item.get("html_url").and_then(|v| v.as_str()))
                .unwrap_or("").to_string();
            items.push(crate::v5::adapters::RemoteRepo {
                name,
                url,
                default_branch: item.get("default_branch").and_then(|v| v.as_str()).map(String::from),
                description: item.get("description").and_then(|v| v.as_str()).map(String::from),
                archived: item.get("archived").and_then(|v| v.as_bool()),
                visibility: if item.get("private").and_then(|v| v.as_bool()).unwrap_or(false) {
                    Some("private".into())
                } else {
                    Some("public".into())
                },
            });
        }
    }
}
