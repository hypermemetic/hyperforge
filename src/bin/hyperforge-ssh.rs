//! hyperforge-ssh — SSH wrapper that selects the correct identity key per forge
//!
//! Git invokes: `hyperforge-ssh <hostname> <git-upload-pack 'org/repo.git'>`
//! We map hostname -> forge, walk up to find .hyperforge/config.toml -> read org,
//! then look up the SSH key from ~/.config/hyperforge/orgs/{org}.toml.
//! Falls back to plain `ssh` if anything goes wrong.

use std::env;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().collect();

    // args[0] = "hyperforge-ssh"
    // args[1..] = SSH arguments that git passes (hostname, command, etc.)
    let ssh_args: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();

    // Try to find the right key; fall back to plain ssh on any failure
    match find_ssh_key(&ssh_args) {
        Some(key_path) => {
            exec_ssh_with_key(&key_path, &ssh_args);
        }
        None => {
            exec_plain_ssh(&ssh_args);
        }
    }
}

/// Attempt to determine the correct SSH key for this connection
fn find_ssh_key(ssh_args: &[&str]) -> Option<String> {
    // Extract hostname from SSH args
    // Git typically passes: hostname "git-upload-pack '/org/repo.git'"
    let hostname = ssh_args.first()?;

    // Map hostname to forge name
    let forge_name = hostname_to_forge(hostname)?;

    // Walk up from CWD to find .hyperforge/config.toml
    let cwd = env::current_dir().ok()?;
    let config = find_hyperforge_config(&cwd)?;

    // Read org from config
    let org = read_org_from_config(&config)?;

    // Read SSH key from org config: ~/.config/hyperforge/orgs/{org}.toml
    let org_config_path = dirs::home_dir()?
        .join(".config")
        .join("hyperforge")
        .join("orgs")
        .join(format!("{}.toml", org));

    read_ssh_key_from_org_config(&org_config_path, &forge_name)
}

/// Map SSH hostname to forge name
fn hostname_to_forge(hostname: &str) -> Option<String> {
    match hostname {
        "github.com" => Some("github".to_string()),
        "codeberg.org" => Some("codeberg".to_string()),
        "gitlab.com" => Some("gitlab".to_string()),
        _ => {
            // Check for custom hostnames containing forge names
            if hostname.contains("github") {
                Some("github".to_string())
            } else if hostname.contains("codeberg") {
                Some("codeberg".to_string())
            } else if hostname.contains("gitlab") {
                Some("gitlab".to_string())
            } else {
                None
            }
        }
    }
}

/// Walk up from `start` to find .hyperforge/config.toml
fn find_hyperforge_config(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let config_path = current.join(".hyperforge").join("config.toml");
        if config_path.exists() {
            return Some(config_path);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Read the `org` field from .hyperforge/config.toml
fn read_org_from_config(config_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(config_path).ok()?;
    // Simple TOML parsing — just find org = "..."
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("org") {
            if let Some(value) = line.split('=').nth(1) {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Read SSH key path for a forge from org config TOML
fn read_ssh_key_from_org_config(config_path: &Path, forge: &str) -> Option<String> {
    let content = std::fs::read_to_string(config_path).ok()?;

    // Look for [ssh] section or ssh.{forge} = "path"
    let mut in_ssh_section = false;
    for line in content.lines() {
        let line = line.trim();

        if line == "[ssh]" {
            in_ssh_section = true;
            continue;
        }
        if line.starts_with('[') {
            in_ssh_section = false;
            continue;
        }

        if in_ssh_section && line.starts_with(forge) {
            if let Some(value) = line.split('=').nth(1) {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    // Expand ~ to home dir
                    let expanded = if value.starts_with("~/") {
                        if let Some(home) = dirs::home_dir() {
                            home.join(&value[2..]).to_string_lossy().to_string()
                        } else {
                            value.to_string()
                        }
                    } else {
                        value.to_string()
                    };
                    return Some(expanded);
                }
            }
        }
    }

    // Also check per-repo config ssh keys (inline ssh.{forge} = "path" format)
    // Try looking for ssh keys in the repo's .hyperforge/config.toml
    None
}

/// Execute ssh with the specified identity key (does not return)
fn exec_ssh_with_key(key_path: &str, ssh_args: &[&str]) -> ! {
    let err = Command::new("ssh")
        .arg("-i")
        .arg(key_path)
        .arg("-o")
        .arg("IdentitiesOnly=yes")
        .args(ssh_args)
        .exec();

    // exec() only returns on error
    eprintln!("hyperforge-ssh: failed to exec ssh: {}", err);
    std::process::exit(1);
}

/// Execute plain ssh without any identity key (does not return)
fn exec_plain_ssh(ssh_args: &[&str]) -> ! {
    let err = Command::new("ssh")
        .args(ssh_args)
        .exec();

    eprintln!("hyperforge-ssh: failed to exec ssh: {}", err);
    std::process::exit(1);
}
