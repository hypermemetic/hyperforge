//! Cabal/Hackage package publishing
//!
//! Publishes Haskell packages to Hackage using `cabal upload`

use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;

use crate::types::VersionBump;
use super::PackageRegistry;

/// Cabal package publisher for Hackage
pub struct CabalPublisher {
    /// Hackage username
    username: Option<String>,
    /// Hackage password
    password: Option<String>,
}

impl CabalPublisher {
    /// Create a new CabalPublisher using default credentials (~/.cabal/config)
    pub fn new() -> Self {
        Self {
            username: None,
            password: None,
        }
    }

    /// Create a CabalPublisher with explicit credentials
    pub fn with_credentials(username: String, password: String) -> Self {
        Self {
            username: Some(username),
            password: Some(password),
        }
    }
}

impl Default for CabalPublisher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PackageRegistry for CabalPublisher {
    async fn detect(&self, path: &Path) -> anyhow::Result<bool> {
        // Look for any .cabal file
        let entries = std::fs::read_dir(path)?;
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".cabal") {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    async fn bump_version(&self, path: &Path, bump: VersionBump) -> anyhow::Result<String> {
        // Find the .cabal file
        let cabal_file = find_cabal_file(path)?;
        let content = tokio::fs::read_to_string(&cabal_file).await?;

        // Parse and bump version
        let (new_content, new_version) = bump_cabal_version(&content, &bump)?;

        tokio::fs::write(&cabal_file, new_content).await?;
        Ok(new_version)
    }

    async fn publish(&self, path: &Path, dry_run: bool) -> anyhow::Result<()> {
        // First, create the sdist
        let mut sdist_cmd = Command::new("cabal");
        sdist_cmd.arg("sdist");
        sdist_cmd.current_dir(path);

        let sdist_output = sdist_cmd.output().await?;
        if !sdist_output.status.success() {
            let stderr = String::from_utf8_lossy(&sdist_output.stderr);
            anyhow::bail!("cabal sdist failed: {}", stderr);
        }

        // Find the generated tarball
        let stdout = String::from_utf8_lossy(&sdist_output.stdout);
        let tarball_path = extract_tarball_path(&stdout, path)?;

        if dry_run {
            // For dry run, just verify the tarball was created
            if !tarball_path.exists() {
                anyhow::bail!("Dry run: tarball not found at {:?}", tarball_path);
            }
            return Ok(());
        }

        // Upload to Hackage
        let mut upload_cmd = Command::new("cabal");
        upload_cmd.arg("upload");
        upload_cmd.arg(&tarball_path);
        upload_cmd.arg("--publish"); // Publish immediately (not as candidate)
        upload_cmd.current_dir(path);

        // Add credentials if provided
        if let (Some(ref user), Some(ref pass)) = (&self.username, &self.password) {
            upload_cmd.arg("--username").arg(user);
            upload_cmd.arg("--password").arg(pass);
        }

        let upload_output = upload_cmd.output().await?;
        if !upload_output.status.success() {
            let stderr = String::from_utf8_lossy(&upload_output.stderr);
            let stdout = String::from_utf8_lossy(&upload_output.stdout);
            anyhow::bail!(
                "cabal upload failed:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            );
        }

        Ok(())
    }
}

/// Find the .cabal file in a directory
fn find_cabal_file(path: &Path) -> anyhow::Result<std::path::PathBuf> {
    let entries = std::fs::read_dir(path)?;
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            if name.ends_with(".cabal") {
                return Ok(entry.path());
            }
        }
    }
    anyhow::bail!("No .cabal file found in {:?}", path)
}

/// Extract the tarball path from cabal sdist output
fn extract_tarball_path(output: &str, base_path: &Path) -> anyhow::Result<std::path::PathBuf> {
    // cabal sdist outputs something like:
    // Wrote sdist tarball to /path/to/dist-newstyle/sdist/package-0.1.0.tar.gz
    for line in output.lines() {
        if line.contains("Wrote sdist tarball to") || line.contains(".tar.gz") {
            // Try to extract the path
            if let Some(path_start) = line.rfind('/') {
                let tarball_name = &line[path_start + 1..];
                // Look in dist-newstyle/sdist/
                let tarball_path = base_path
                    .join("dist-newstyle")
                    .join("sdist")
                    .join(tarball_name.trim());
                if tarball_path.exists() {
                    return Ok(tarball_path);
                }
            }
        }
    }

    // Fallback: search dist-newstyle/sdist/ for any .tar.gz
    let sdist_dir = base_path.join("dist-newstyle").join("sdist");
    if sdist_dir.exists() {
        for entry in std::fs::read_dir(&sdist_dir)?.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".tar.gz") {
                    return Ok(entry.path());
                }
            }
        }
    }

    anyhow::bail!("Could not find sdist tarball in output: {}", output)
}

/// Bump version in a .cabal file
fn bump_cabal_version(content: &str, bump: &VersionBump) -> anyhow::Result<(String, String)> {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let mut new_version = String::new();

    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.to_lowercase().starts_with("version:") {
            // Extract current version
            let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
            if parts.len() == 2 {
                let current_version = parts[1].trim();
                new_version = bump_pep440_version(current_version, bump)?;

                // Preserve indentation
                let indent = line.len() - trimmed.len();
                *line = format!("{}version: {}", " ".repeat(indent), new_version);
            }
            break;
        }
    }

    if new_version.is_empty() {
        anyhow::bail!("No version field found in .cabal file");
    }

    Ok((lines.join("\n"), new_version))
}

/// Bump a PEP440-style version (also works for Haskell's PVP with adjustment)
fn bump_pep440_version(version: &str, bump: &VersionBump) -> anyhow::Result<String> {
    let parts: Vec<&str> = version.split('.').collect();

    // Handle both 3-part (semver) and 4-part (PVP) versions
    match parts.len() {
        3 => {
            let major: u64 = parts[0].parse()?;
            let minor: u64 = parts[1].parse()?;
            let patch: u64 = parts[2].parse()?;

            let (new_major, new_minor, new_patch) = match bump {
                VersionBump::Major => (major + 1, 0, 0),
                VersionBump::Minor => (major, minor + 1, 0),
                VersionBump::Patch => (major, minor, patch + 1),
            };

            Ok(format!("{}.{}.{}", new_major, new_minor, new_patch))
        }
        4 => {
            // PVP: A.B.C.D where A.B is major, C is minor, D is patch
            let a: u64 = parts[0].parse()?;
            let b: u64 = parts[1].parse()?;
            let c: u64 = parts[2].parse()?;
            let d: u64 = parts[3].parse()?;

            let (new_a, new_b, new_c, new_d) = match bump {
                VersionBump::Major => (a, b + 1, 0, 0),
                VersionBump::Minor => (a, b, c + 1, 0),
                VersionBump::Patch => (a, b, c, d + 1),
            };

            Ok(format!("{}.{}.{}.{}", new_a, new_b, new_c, new_d))
        }
        _ => anyhow::bail!("Unsupported version format: {}", version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bump_pep440_patch() {
        assert_eq!(
            bump_pep440_version("1.2.3", &VersionBump::Patch).unwrap(),
            "1.2.4"
        );
    }

    #[test]
    fn test_bump_pvp_patch() {
        assert_eq!(
            bump_pep440_version("0.1.0.0", &VersionBump::Patch).unwrap(),
            "0.1.0.1"
        );
    }

    #[test]
    fn test_bump_pvp_minor() {
        assert_eq!(
            bump_pep440_version("0.1.0.0", &VersionBump::Minor).unwrap(),
            "0.1.1.0"
        );
    }

    #[test]
    fn test_bump_pvp_major() {
        assert_eq!(
            bump_pep440_version("0.1.0.0", &VersionBump::Major).unwrap(),
            "0.2.0.0"
        );
    }

    #[test]
    fn test_bump_cabal_version() {
        let content = r#"name: my-package
version: 0.1.0.0
synopsis: A test package
"#;
        let (new_content, new_version) =
            bump_cabal_version(content, &VersionBump::Patch).unwrap();
        assert_eq!(new_version, "0.1.0.1");
        assert!(new_content.contains("version: 0.1.0.1"));
    }
}
