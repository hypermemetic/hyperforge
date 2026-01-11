use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceBinding {
    pub path: PathBuf,
    pub org_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceResolution {
    pub org_name: String,
    pub source: ResolutionSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionSource {
    Explicit,
    LocalConfig,
    WorkspaceBinding { path: PathBuf },
    Default,
}
