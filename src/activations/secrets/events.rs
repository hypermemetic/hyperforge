//! Event types for secrets activations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::SecretKey;

// ============================================================================
// SecretEvent
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretEvent {
    /// List of secret keys
    Listed {
        org_name: String,
        keys: Vec<SecretKey>,
    },

    /// Secret value retrieved
    Retrieved {
        org_name: String,
        key: String,
        value: String,
    },

    /// Secret value set
    Updated { org_name: String, key: String },

    /// Prompt for secret value (interactive)
    PromptRequired {
        org_name: String,
        key: String,
        message: String,
    },

    /// Secret acquisition started
    AcquireStarted { org_name: String, forge: String },

    /// Secret acquired successfully
    Acquired {
        org_name: String,
        key: String,
        source: String,
    },

    /// Error during secret operation
    Error {
        org_name: String,
        key: Option<String>,
        message: String,
    },
}
