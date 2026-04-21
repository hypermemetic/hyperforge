//! Cross-compilation and binary packaging engine
//!
//! Supports compiling Rust projects for multiple target triples (native via cargo,
//! cross-compilation via `cross`) and Haskell projects (native only via cabal).
//! Packages resulting binaries into binstall-compatible archives.

use std::path::{Path, PathBuf};

use super::BuildSystemKind;

/// Archive format for packaged binaries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarGz,
    Zip,
    TarXz,
}

impl ArchiveFormat {
    /// File extension for this archive format
    pub const fn extension(&self) -> &str {
        match self {
            Self::TarGz => ".tar.gz",
            Self::Zip => ".zip",
            Self::TarXz => ".tar.xz",
        }
    }
}

/// A target triple for cross-compilation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetTriple {
    pub triple: String,
}

impl TargetTriple {
    /// Create a new `TargetTriple` from a string
    pub fn new(triple: impl Into<String>) -> Self {
        Self {
            triple: triple.into(),
        }
    }

    /// Determine the appropriate archive format for this target
    pub fn archive_format(&self) -> ArchiveFormat {
        if self.is_windows() {
            ArchiveFormat::Zip
        } else {
            ArchiveFormat::TarGz
        }
    }

    /// Whether this target is a Windows target
    pub fn is_windows(&self) -> bool {
        self.triple.contains("windows")
    }

    /// Whether this target matches the current host
    pub fn is_native(&self) -> bool {
        self.triple == host_triple()
    }

    /// Binary file extension for this target ("" or ".exe")
    pub fn binary_extension(&self) -> &str {
        if self.is_windows() {
            ".exe"
        } else {
            ""
        }
    }
}

impl From<&str> for TargetTriple {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl std::fmt::Display for TargetTriple {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.triple)
    }
}

/// Common cross-compilation targets
pub const COMMON_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

/// Linux-specific targets
pub const LINUX_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

/// Apple-specific targets
pub const APPLE_TARGETS: &[&str] = &[
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
];

/// Result of compiling for a single target
#[derive(Debug, Clone)]
pub struct CompileResult {
    /// The target that was compiled for
    pub target: TargetTriple,
    /// Path to the created archive, if packaging succeeded
    pub archive_path: Option<PathBuf>,
    /// Whether compilation succeeded
    pub success: bool,
    /// Error message if compilation failed
    pub error: Option<String>,
}

/// Get the host target triple
pub fn host_triple() -> String {
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "i686"
    } else {
        "unknown"
    };

    let os = if cfg!(target_os = "linux") {
        "unknown-linux-gnu"
    } else if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(target_os = "windows") {
        "pc-windows-msvc"
    } else {
        "unknown-unknown"
    };

    format!("{arch}-{os}")
}

/// Compile a project for a specific target triple.
///
/// For Rust: uses `cargo build` for native targets, `cross build` for cross-compilation.
/// For Haskell: uses `cabal build -O2` for native targets only.
///
/// Returns paths to the compiled binary files.
pub async fn compile_for_target(
    repo_path: &Path,
    build_system: &BuildSystemKind,
    target: &TargetTriple,
    binary_names: &[String],
) -> Result<Vec<PathBuf>, String> {
    match build_system {
        BuildSystemKind::Cargo => compile_rust(repo_path, target, binary_names).await,
        BuildSystemKind::Cabal => compile_haskell(repo_path, target, binary_names).await,
        other => Err(format!(
            "cross-compilation not supported for {other} projects"
        )),
    }
}

async fn compile_rust(
    repo_path: &Path,
    target: &TargetTriple,
    binary_names: &[String],
) -> Result<Vec<PathBuf>, String> {
    let (program, mut args) = if target.is_native() {
        ("cargo", vec!["build", "--release", "--target", &target.triple])
    } else {
        ("cross", vec!["build", "--release", "--target", &target.triple])
    };

    // Add --bin flags for each binary
    for name in binary_names {
        args.push("--bin");
        args.push(name);
    }

    let output = tokio::process::Command::new(program)
        .args(&args)
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| format!("failed to spawn {program}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} build failed for {}: {}",
            program, target.triple, stderr
        ));
    }

    // Collect compiled binary paths
    let ext = target.binary_extension();
    let mut binaries = Vec::new();
    for name in binary_names {
        let binary_path = repo_path
            .join("target")
            .join(&target.triple)
            .join("release")
            .join(format!("{name}{ext}"));
        if binary_path.exists() {
            binaries.push(binary_path);
        } else {
            return Err(format!(
                "expected binary not found: {}",
                binary_path.display()
            ));
        }
    }

    Ok(binaries)
}

async fn compile_haskell(
    repo_path: &Path,
    target: &TargetTriple,
    binary_names: &[String],
) -> Result<Vec<PathBuf>, String> {
    if !target.is_native() {
        return Err("cross-compilation not supported for Haskell".to_string());
    }

    let output = tokio::process::Command::new("cabal")
        .args(["build", "-O2"])
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| format!("failed to spawn cabal: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cabal build failed: {stderr}"));
    }

    // Use `cabal list-bin` to find each binary
    let mut binaries = Vec::new();
    for name in binary_names {
        let list_bin = tokio::process::Command::new("cabal")
            .args(["list-bin", name])
            .current_dir(repo_path)
            .output()
            .await
            .map_err(|e| format!("failed to run cabal list-bin: {e}"))?;

        if !list_bin.status.success() {
            return Err(format!(
                "cabal list-bin {} failed: {}",
                name,
                String::from_utf8_lossy(&list_bin.stderr)
            ));
        }

        let path_str = String::from_utf8_lossy(&list_bin.stdout).trim().to_string();
        let binary_path = PathBuf::from(&path_str);
        if binary_path.exists() {
            binaries.push(binary_path);
        } else {
            return Err(format!("binary not found at: {path_str}"));
        }
    }

    Ok(binaries)
}

/// Package compiled binaries into a binstall-compatible archive.
///
/// Creates an archive with the naming convention:
///   `{name}-{target}-v{version}.tar.gz` (or .zip for Windows)
///
/// Internal structure:
///   `{name}-{target}-v{version}/{binary}{ext}`
///
/// Returns the path to the created archive.
pub fn package_binaries(
    binary_paths: &[PathBuf],
    target: &TargetTriple,
    package_name: &str,
    version: &str,
    output_dir: &Path,
) -> Result<PathBuf, String> {
    let format = target.archive_format();
    let stem = format!("{}-{}-v{}", package_name, target.triple, version);
    let archive_name = format!("{}{}", stem, format.extension());
    let archive_path = output_dir.join(&archive_name);

    match format {
        ArchiveFormat::TarGz => {
            create_tar_gz(binary_paths, &stem, target, &archive_path)?;
        }
        ArchiveFormat::Zip => {
            // Zip support deferred — create tar.gz as fallback
            create_tar_gz(binary_paths, &stem, target, &archive_path)?;
        }
        ArchiveFormat::TarXz => {
            return Err("tar.xz packaging not yet implemented".to_string());
        }
    }

    Ok(archive_path)
}

fn create_tar_gz(
    binary_paths: &[PathBuf],
    stem: &str,
    target: &TargetTriple,
    archive_path: &Path,
) -> Result<(), String> {
    let file = std::fs::File::create(archive_path)
        .map_err(|e| format!("failed to create archive file: {e}"))?;
    let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);

    for binary_path in binary_paths {
        let file_name = binary_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "invalid binary filename".to_string())?;

        // Apply target extension if not already present
        let ext = target.binary_extension();
        let entry_name = if !ext.is_empty() && !file_name.ends_with(ext) {
            format!("{stem}/{file_name}{ext}")
        } else {
            format!("{stem}/{file_name}")
        };

        tar.append_path_with_name(binary_path, &entry_name)
            .map_err(|e| format!("failed to add {file_name} to archive: {e}"))?;
    }

    tar.into_inner()
        .map_err(|e| format!("failed to finalize tar: {e}"))?
        .finish()
        .map_err(|e| format!("failed to finalize gzip: {e}"))?;

    Ok(())
}

/// Compile and package binaries for multiple targets.
///
/// Orchestrates `compile_for_target` + `package_binaries` for each target,
/// returning a `CompileResult` per target.
pub async fn compile_and_package(
    repo_path: &Path,
    build_system: &BuildSystemKind,
    targets: &[TargetTriple],
    binary_names: &[String],
    version: &str,
    output_dir: &Path,
) -> Vec<CompileResult> {
    // Ensure output directory exists
    if let Err(e) = std::fs::create_dir_all(output_dir) {
        return targets
            .iter()
            .map(|t| CompileResult {
                target: t.clone(),
                archive_path: None,
                success: false,
                error: Some(format!("failed to create output dir: {e}")),
            })
            .collect();
    }

    let mut results = Vec::new();

    for target in targets {
        let result =
            match compile_for_target(repo_path, build_system, target, binary_names).await {
                Ok(binaries) => {
                    let pkg_name = repo_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");

                    match package_binaries(&binaries, target, pkg_name, version, output_dir) {
                        Ok(archive_path) => CompileResult {
                            target: target.clone(),
                            archive_path: Some(archive_path),
                            success: true,
                            error: None,
                        },
                        Err(e) => CompileResult {
                            target: target.clone(),
                            archive_path: None,
                            success: false,
                            error: Some(format!("packaging failed: {e}")),
                        },
                    }
                }
                Err(e) => CompileResult {
                    target: target.clone(),
                    archive_path: None,
                    success: false,
                    error: Some(e),
                },
            };

        results.push(result);
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_target_triple_from_str() {
        let target: TargetTriple = "x86_64-unknown-linux-gnu".into();
        assert_eq!(target.triple, "x86_64-unknown-linux-gnu");
    }

    #[test]
    fn test_target_triple_display() {
        let target = TargetTriple::new("aarch64-apple-darwin");
        assert_eq!(format!("{target}"), "aarch64-apple-darwin");
    }

    #[test]
    fn test_is_windows() {
        assert!(TargetTriple::new("x86_64-pc-windows-msvc").is_windows());
        assert!(TargetTriple::new("aarch64-pc-windows-msvc").is_windows());
        assert!(!TargetTriple::new("x86_64-unknown-linux-gnu").is_windows());
        assert!(!TargetTriple::new("aarch64-apple-darwin").is_windows());
    }

    #[test]
    fn test_archive_format_windows_is_zip() {
        let win = TargetTriple::new("x86_64-pc-windows-msvc");
        assert_eq!(win.archive_format(), ArchiveFormat::Zip);
    }

    #[test]
    fn test_archive_format_linux_is_targz() {
        let linux = TargetTriple::new("x86_64-unknown-linux-gnu");
        assert_eq!(linux.archive_format(), ArchiveFormat::TarGz);
    }

    #[test]
    fn test_archive_format_macos_is_targz() {
        let mac = TargetTriple::new("aarch64-apple-darwin");
        assert_eq!(mac.archive_format(), ArchiveFormat::TarGz);
    }

    #[test]
    fn test_binary_extension_windows() {
        let win = TargetTriple::new("x86_64-pc-windows-msvc");
        assert_eq!(win.binary_extension(), ".exe");
    }

    #[test]
    fn test_binary_extension_unix() {
        let linux = TargetTriple::new("x86_64-unknown-linux-gnu");
        assert_eq!(linux.binary_extension(), "");

        let mac = TargetTriple::new("aarch64-apple-darwin");
        assert_eq!(mac.binary_extension(), "");
    }

    #[test]
    fn test_is_native_matches_host() {
        let host = host_triple();
        let native = TargetTriple::new(&host);
        assert!(native.is_native());

        // A non-host triple should not be native
        let other = if host.contains("linux") {
            "aarch64-apple-darwin"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        assert!(!TargetTriple::new(other).is_native());
    }

    #[test]
    fn test_host_triple_is_valid() {
        let host = host_triple();
        assert!(!host.is_empty());
        assert!(host.contains('-'));
        // Should be a recognized platform
        assert!(
            host.contains("linux") || host.contains("darwin") || host.contains("windows"),
            "unexpected host triple: {host}"
        );
    }

    #[test]
    fn test_common_targets_coverage() {
        assert_eq!(COMMON_TARGETS.len(), 5);
        assert!(COMMON_TARGETS.contains(&"x86_64-unknown-linux-gnu"));
        assert!(COMMON_TARGETS.contains(&"aarch64-apple-darwin"));
        assert!(COMMON_TARGETS.contains(&"x86_64-pc-windows-msvc"));
    }

    #[test]
    fn test_archive_format_extensions() {
        assert_eq!(ArchiveFormat::TarGz.extension(), ".tar.gz");
        assert_eq!(ArchiveFormat::Zip.extension(), ".zip");
        assert_eq!(ArchiveFormat::TarXz.extension(), ".tar.xz");
    }

    #[test]
    fn test_package_binaries_creates_tar_gz() {
        let tmp = TempDir::new().unwrap();
        let output_dir = tmp.path().join("dist");
        std::fs::create_dir_all(&output_dir).unwrap();

        // Create fake binary files
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin1 = bin_dir.join("myapp");
        let bin2 = bin_dir.join("myapp-helper");
        std::fs::write(&bin1, b"fake binary 1").unwrap();
        std::fs::write(&bin2, b"fake binary 2").unwrap();

        let target = TargetTriple::new("x86_64-unknown-linux-gnu");
        let result =
            package_binaries(&[bin1, bin2], &target, "myapp", "1.2.3", &output_dir);

        assert!(result.is_ok());
        let archive_path = result.unwrap();
        assert!(archive_path.exists());
        assert_eq!(
            archive_path.file_name().unwrap().to_str().unwrap(),
            "myapp-x86_64-unknown-linux-gnu-v1.2.3.tar.gz"
        );

        // Verify archive contents
        let file = std::fs::File::open(&archive_path).unwrap();
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);
        let entries: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|e| e.path().unwrap().to_string_lossy().to_string())
            .collect();

        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|e| e == "myapp-x86_64-unknown-linux-gnu-v1.2.3/myapp"));
        assert!(entries
            .iter()
            .any(|e| e == "myapp-x86_64-unknown-linux-gnu-v1.2.3/myapp-helper"));
    }

    #[test]
    fn test_package_binaries_single_binary() {
        let tmp = TempDir::new().unwrap();
        let output_dir = tmp.path().join("dist");
        std::fs::create_dir_all(&output_dir).unwrap();

        let bin_path = tmp.path().join("tool");
        std::fs::write(&bin_path, b"binary content").unwrap();

        let target = TargetTriple::new("aarch64-apple-darwin");
        let result =
            package_binaries(&[bin_path], &target, "tool", "0.1.0", &output_dir).unwrap();

        assert!(result.exists());
        assert_eq!(
            result.file_name().unwrap().to_str().unwrap(),
            "tool-aarch64-apple-darwin-v0.1.0.tar.gz"
        );

        // Verify single entry
        let file = std::fs::File::open(&result).unwrap();
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);
        let entries: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|e| e.path().unwrap().to_string_lossy().to_string())
            .collect();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], "tool-aarch64-apple-darwin-v0.1.0/tool");
    }

    #[test]
    fn test_package_binaries_missing_file_fails() {
        let tmp = TempDir::new().unwrap();
        let output_dir = tmp.path().join("dist");
        std::fs::create_dir_all(&output_dir).unwrap();

        let missing = tmp.path().join("nonexistent");
        let target = TargetTriple::new("x86_64-unknown-linux-gnu");
        let result = package_binaries(&[missing], &target, "app", "1.0.0", &output_dir);
        assert!(result.is_err());
    }
}
