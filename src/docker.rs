//! Docker daemon connection with automatic socket detection.
//!
//! Handles Colima, Docker Desktop, and standard Linux sockets by reading
//! Docker's config files directly — no subprocess calls.
//!
//! Logging targets:
//!   - `hyperforge::docker::connect` — socket discovery
//!   - `hyperforge::docker::build` — image builds
//!   - `hyperforge::docker::push` — registry push

use bollard::Docker;
use tracing::{debug, info, warn, trace};

/// Docker daemon availability state
#[derive(Debug, Clone)]
pub enum DockerState {
    Available { version: String },
    NotRunning,
    NotInstalled,
}

/// Discover the Docker host socket by reading Docker config files.
/// Priority: DOCKER_HOST env → ~/.docker/config.json context → bollard default.
fn discover_docker_host() -> Option<String> {
    // 1. DOCKER_HOST env takes priority
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if !host.is_empty() {
            debug!(target: "hyperforge::docker::connect", host = %host, "Using DOCKER_HOST from environment");
            return Some(host);
        }
    }

    // 2. Read active Docker context from ~/.docker/config.json
    let docker_dir = dirs::home_dir()?.join(".docker");
    let config_path = docker_dir.join("config.json");
    let config_str = std::fs::read_to_string(&config_path).ok()?;

    let ctx_name = config_str
        .split("\"currentContext\"")
        .nth(1)?
        .split('"')
        .nth(1)?;

    if ctx_name == "default" {
        debug!(target: "hyperforge::docker::connect", "Docker context is 'default', using bollard defaults");
        return None;
    }

    // 3. Scan context metadata dirs for matching name
    let meta_dir = docker_dir.join("contexts").join("meta");
    let entries = std::fs::read_dir(&meta_dir).ok()?;
    for entry in entries.flatten() {
        let meta_path = entry.path().join("meta.json");
        if let Ok(meta_str) = std::fs::read_to_string(&meta_path) {
            let name_match = format!("\"Name\":\"{}\"", ctx_name);
            if meta_str.contains(&name_match) {
                if let Some(host) = meta_str
                    .split("\"Host\":\"")
                    .nth(1)
                    .and_then(|s| s.split('"').next())
                {
                    info!(target: "hyperforge::docker::connect", context = %ctx_name, host = %host, "Discovered Docker socket from context");
                    return Some(host.to_string());
                }
            }
        }
    }
    None
}

/// Connect to the Docker daemon, auto-detecting the socket path.
pub fn connect() -> Result<Docker, String> {
    if let Some(host) = discover_docker_host() {
        let sock_path = host.strip_prefix("unix://").unwrap_or(&host);
        Docker::connect_with_unix(sock_path, 120, bollard::API_DEFAULT_VERSION)
            .map_err(|e| format!("Failed to connect to Docker at {}: {}", sock_path, e))
    } else {
        Docker::connect_with_local_defaults()
            .map_err(|e| format!("Failed to connect to Docker: {}", e))
    }
}

/// Check if Docker is available and return its state.
pub async fn check_state(docker: &Docker) -> DockerState {
    match docker.version().await {
        Ok(version) => {
            let ver = version.version.unwrap_or_else(|| "unknown".into());
            DockerState::Available { version: ver }
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("No such file") || msg.contains("connect") {
                DockerState::NotRunning
            } else {
                DockerState::NotInstalled
            }
        }
    }
}

/// Build a Docker image from a Dockerfile.
/// Returns the image ID on success.
pub async fn build_image(
    docker: &Docker,
    build_context: &std::path::Path,
    dockerfile: &str,
    image_tag: &str,
) -> Result<String, String> {
    use bollard::image::BuildImageOptions;
    use futures::StreamExt;

    // Create a tar archive of the build context
    info!(target: "hyperforge::docker::build", tag = %image_tag, dockerfile = %dockerfile, context = %build_context.display(), "Starting Docker build");
    let tar_data = create_build_tar(build_context, dockerfile)?;
    debug!(target: "hyperforge::docker::build", size_bytes = tar_data.len(), "Build context tar created");

    let options = BuildImageOptions {
        t: image_tag,
        dockerfile,
        rm: true,
        ..Default::default()
    };

    let mut stream = docker.build_image(options, None, Some(tar_data.into()));
    let mut last_id = String::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(info) => {
                if let Some(ref id) = info.id {
                    last_id = id.clone();
                }
                if let Some(ref stream_line) = info.stream {
                    trace!(target: "hyperforge::docker::build", "{}", stream_line.trim());
                }
                if let Some(error) = info.error {
                    warn!(target: "hyperforge::docker::build", error = %error, "Build error");
                    return Err(format!("Build error: {}", error));
                }
            }
            Err(e) => return Err(format!("Build stream error: {}", e)),
        }
    }

    Ok(last_id)
}

/// Tag a local image with a remote reference.
pub async fn tag_image(
    docker: &Docker,
    source: &str,
    repo: &str,
    tag: &str,
) -> Result<(), String> {
    use bollard::image::TagImageOptions;

    docker.tag_image(source, Some(TagImageOptions { repo, tag }))
        .await
        .map_err(|e| format!("Failed to tag image: {}", e))
}

/// Push an image to a registry.
pub async fn push_image(
    docker: &Docker,
    image: &str,
    tag: &str,
    credentials: bollard::auth::DockerCredentials,
) -> Result<(), String> {
    use bollard::image::PushImageOptions;
    use futures::StreamExt;

    info!(target: "hyperforge::docker::push", image = %image, tag = %tag, "Pushing image to registry");
    let options = PushImageOptions { tag };

    let mut stream = docker.push_image(image, Some(options), Some(credentials));

    while let Some(result) = stream.next().await {
        match result {
            Ok(info) => {
                if let Some(ref status) = info.status {
                    trace!(target: "hyperforge::docker::push", status = %status, "Push progress");
                }
                if let Some(error) = info.error {
                    warn!(target: "hyperforge::docker::push", error = %error, "Push error");
                    return Err(format!("Push error: {}", error));
                }
            }
            Err(e) => return Err(format!("Push stream error: {}", e)),
        }
    }

    Ok(())
}

/// Convert our RegistryAuth to bollard DockerCredentials.
pub fn to_docker_credentials(
    auth: &crate::types::RegistryAuth,
    registry: &crate::types::ContainerRegistry,
    org: &str,
) -> bollard::auth::DockerCredentials {
    match auth {
        crate::types::RegistryAuth::Token(token) => bollard::auth::DockerCredentials {
            username: Some(org.to_string()),
            password: Some(token.clone()),
            serveraddress: Some(format!("https://{}", registry.host())),
            ..Default::default()
        },
        crate::types::RegistryAuth::Basic { username, password } => bollard::auth::DockerCredentials {
            username: Some(username.clone()),
            password: Some(password.clone()),
            serveraddress: Some(format!("https://{}", registry.host())),
            ..Default::default()
        },
        crate::types::RegistryAuth::Anonymous => bollard::auth::DockerCredentials::default(),
    }
}

/// Create a tar archive from a build context directory.
fn create_build_tar(context_path: &std::path::Path, _dockerfile: &str) -> Result<Vec<u8>, String> {
    let buf = Vec::new();
    let mut archive = tar::Builder::new(buf);

    archive.append_dir_all(".", context_path)
        .map_err(|e| format!("Failed to create build context tar: {}", e))?;

    archive.into_inner()
        .map_err(|e| format!("Failed to finalize tar: {}", e))
}
