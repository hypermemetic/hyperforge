//! RPC-based auth provider
//!
//! Calls the auth hub via synapse to get secrets.
//! This is a workaround that demonstrates proper separation - hyperforge
//! doesn't know about YAML storage, it just calls the auth service via RPC.

use async_trait::async_trait;
use serde::Deserialize;
use std::process::Command;

use super::AuthProvider;

/// Auth event from auth hub
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AuthEvent {
    #[serde(rename = "secret")]
    Secret {
        path: String,
        value: String,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
    },
}

/// RPC-based auth provider that calls auth hub via synapse
pub struct YamlAuthProvider {
    auth_port: u16,
}

impl YamlAuthProvider {
    /// Create a new RPC auth provider
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            auth_port: 4445,
        })
    }

    /// Create with custom auth port
    pub fn with_port(port: u16) -> anyhow::Result<Self> {
        Ok(Self {
            auth_port: port,
        })
    }
}

#[async_trait]
impl AuthProvider for YamlAuthProvider {
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<String>> {
        // Call auth hub via synapse (uses PATH to find synapse binary)
        // synapse -P 4445 auth auth get_secret --path <key> --raw
        let output = Command::new("synapse")
            .args(&[
                "-P",
                &self.auth_port.to_string(),
                "auth",
                "auth",
                "get_secret",
                "--path",
                key,
                "--raw",
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Check if it's a "not found" error
            if stderr.contains("Secret not found") || stderr.contains("Not found") {
                return Ok(None);
            }
            return Err(anyhow::anyhow!("Failed to get secret from auth hub: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse the JSON response
        let event: AuthEvent = serde_json::from_str(stdout.trim())
            .map_err(|e| anyhow::anyhow!("Failed to parse auth response: {}", e))?;

        match event {
            AuthEvent::Secret { value, .. } => Ok(Some(value)),
            AuthEvent::Error { message } => {
                if message.contains("not found") {
                    Ok(None)
                } else {
                    Err(anyhow::anyhow!("Auth error: {}", message))
                }
            }
        }
    }
}
