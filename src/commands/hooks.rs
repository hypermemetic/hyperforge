//! Git hook templates for hyperforge
//!
//! Contains hook scripts that validate push targets against declared
//! .hyperforge/config.toml settings.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Pre-push hook script template
///
/// This script is installed to `.hyperforge/hooks/pre-push` and validates:
/// 1. The push target forge is in the repo's declared forges list
/// 2. The push target org matches the repo's declared org (or forge-specific override)
///
/// Uses Python's tomllib (3.11+) for proper TOML parsing. Falls back to
/// allowing the push if Python 3 is unavailable.
pub const PRE_PUSH_HOOK: &str = r#"#!/bin/sh
# hyperforge pre-push hook
# Validates push targets against .hyperforge/config.toml
# Installed by: hyperforge init

REMOTE="$1"
URL="$2"

# Find .hyperforge/config.toml relative to repo root
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
CONFIG="$REPO_ROOT/.hyperforge/config.toml"

if [ ! -f "$CONFIG" ]; then
    # No config — allow push (not a hyperforge-managed repo)
    exit 0
fi

# Determine forge from URL
FORGE=""
case "$URL" in
    *github.com*)    FORGE="github" ;;
    *codeberg.org*)  FORGE="codeberg" ;;
    *gitlab.com*)    FORGE="gitlab" ;;
esac

if [ -z "$FORGE" ]; then
    # Unknown forge — allow push (could be a custom remote)
    exit 0
fi

# Extract org from push URL
URL_ORG=""
case "$URL" in
    git@*)
        # git@github.com:org/repo.git
        URL_ORG=$(echo "$URL" | sed 's/.*:\(.*\)\/.*/\1/')
        ;;
    https://*)
        # https://github.com/org/repo.git
        URL_ORG=$(echo "$URL" | sed 's|https://[^/]*/\([^/]*\)/.*|\1|')
        ;;
esac

# Use Python for proper TOML parsing; allow push if python3 unavailable
if ! command -v python3 >/dev/null 2>&1; then
    exit 0
fi

exec python3 -c "
import sys

forge = sys.argv[1]
url_org = sys.argv[2]
config_path = sys.argv[3]

# Parse TOML config
try:
    try:
        import tomllib
    except ImportError:
        try:
            import tomli as tomllib
        except ImportError:
            # No TOML parser — allow push
            sys.exit(0)

    with open(config_path, 'rb') as f:
        config = tomllib.load(f)
except Exception:
    # Parse error — allow push
    sys.exit(0)

# Check forge is in declared list
forges = config.get('forges', [])
if forges and forge not in forges:
    print(f\"hyperforge: BLOCKED — forge '{forge}' not in declared forges {forges}\")
    print('hyperforge: Update .hyperforge/config.toml to add this forge, or use --no-verify to skip.')
    sys.exit(1)

# Check org matches
declared_org = config.get('org', '')
if declared_org and url_org:
    # Check for forge-specific org override
    forge_section = config.get('forge', {}).get(forge, {})
    expected_org = forge_section.get('org', declared_org)

    if url_org != expected_org:
        print(f\"hyperforge: BLOCKED — pushing to org '{url_org}' but declared org is '{expected_org}'\")
        print('hyperforge: This prevents accidental pushes to the wrong organization.')
        print('hyperforge: Use --no-verify to skip, or update .hyperforge/config.toml.')
        sys.exit(1)

# All checks passed
sys.exit(0)
" "$FORGE" "$URL_ORG" "$CONFIG"
"#;

/// Install the pre-push hook to a repo's .hyperforge/hooks/ directory
pub fn install_pre_push_hook(repo_path: &Path, dry_run: bool) -> std::io::Result<bool> {
    let hooks_dir = repo_path.join(".hyperforge").join("hooks");
    let hook_path = hooks_dir.join("pre-push");

    // Check if hook already exists with same content
    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path)?;
        if existing == PRE_PUSH_HOOK {
            return Ok(false); // Already installed, no change
        }
    }

    if dry_run {
        return Ok(true); // Would install
    }

    // Create hooks directory
    fs::create_dir_all(&hooks_dir)?;

    // Write hook
    fs::write(&hook_path, PRE_PUSH_HOOK)?;

    // Make executable (rwxr-xr-x)
    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(&hook_path, perms)?;

    Ok(true) // Installed
}
