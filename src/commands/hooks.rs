//! Git hook templates for hyperforge
//!
//! Contains hook scripts that validate push targets against declared
//! .hyperforge/config.toml settings.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Pre-push hook script template (HYPE-5).
///
/// This script is installed to `.hyperforge/hooks/pre-push` and validates:
/// 1. The push target forge is in the repo's declared forges list
/// 2. The push target org matches the repo's declared org (or forge-specific
///    override), compared by **canonical owner identity** — the embedded
///    `OWNER_ALIASES` table (rendered from `config.yaml`'s `owner_aliases`)
///    and `_canon()` mirror `GlobalConfig::canonical_owner`, so a repo that
///    moved user→org (aliased owner) is not falsely blocked, while a genuine
///    unrelated org mismatch still is.
///
/// The `__OWNER_ALIASES__` marker is replaced by a Python dict literal at
/// install time via [`render_pre_push_hook`].
///
/// Uses Python's tomllib (3.11+) for proper TOML parsing. Falls back to
/// allowing the push if Python 3 is unavailable.
const PRE_PUSH_HOOK_TEMPLATE: &str = r#"#!/bin/sh
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

# --- Large file check (100KB threshold) ---
THRESHOLD=102400
LARGE_FILES=$(git ls-files -z | xargs -0 stat -f '%z %N' 2>/dev/null || git ls-files -z | xargs -0 stat --format='%s %n' 2>/dev/null)
if [ -n "$LARGE_FILES" ]; then
    BLOCKED=""
    echo "$LARGE_FILES" | while IFS= read -r line; do
        SIZE=$(echo "$line" | awk '{print $1}')
        FILE=$(echo "$line" | cut -d' ' -f2-)
        if [ "$SIZE" -gt "$THRESHOLD" ] 2>/dev/null; then
            KB=$(( SIZE / 1024 ))
            echo "hyperforge: large tracked file: $FILE (${KB}KB)" >&2
            echo "BLOCKED" > /tmp/.hyperforge_blocked_$$
        fi
    done
    if [ -f /tmp/.hyperforge_blocked_$$ ]; then
        rm -f /tmp/.hyperforge_blocked_$$
        echo "hyperforge: BLOCKED — large tracked files detected (threshold: 100KB)" >&2
        echo "hyperforge: Remove or .gitignore these files, then commit. Use --no-verify to skip." >&2
        exit 1
    fi
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

# HYPE-5: owner-alias identity. This table + _canon() mirror
# GlobalConfig::canonical_owner — aliased owners resolve to one canonical
# identity so a repo that moved user->org is not falsely blocked.
OWNER_ALIASES = __OWNER_ALIASES__
def _canon(o):
    return OWNER_ALIASES.get(o, o)

# Check org matches (by canonical identity)
declared_org = config.get('org', '')
if declared_org and url_org:
    # Check for forge-specific org override
    forge_section = config.get('forge', {}).get(forge, {})
    expected_org = forge_section.get('org', declared_org)

    if _canon(url_org) != _canon(expected_org):
        print(f\"hyperforge: BLOCKED — pushing to org '{url_org}' but declared org is '{expected_org}'\")
        print('hyperforge: This prevents accidental pushes to the wrong organization.')
        print('hyperforge: Use --no-verify to skip, or update .hyperforge/config.toml.')
        sys.exit(1)

# All checks passed
sys.exit(0)
" "$FORGE" "$URL_ORG" "$CONFIG"
"#;

/// Render the pre-push hook, embedding the `alias -> canonical` owner table
/// (HYPE-5) as the Python `OWNER_ALIASES` dict. An empty table yields `{}`,
/// so the hook is byte-identical in spirit to the pre-alias version when no
/// aliases are configured. The Python literal is emitted with `\"`-escaped
/// keys/values to sit inside the `python3 -c "…"` double-quoted shell arg.
#[must_use]
pub fn render_pre_push_hook(alias_pairs: &BTreeMap<String, String>) -> String {
    let entries: Vec<String> = alias_pairs
        .iter()
        .map(|(alias, canonical)| format!("\\\"{alias}\\\": \\\"{canonical}\\\""))
        .collect();
    let dict = format!("{{{}}}", entries.join(", "));
    PRE_PUSH_HOOK_TEMPLATE.replace("__OWNER_ALIASES__", &dict)
}

/// Would the pre-push hook block a push to `url_org` when the repo declares
/// `expected_org`, given the `alias -> canonical` table? (HYPE-5.) This is
/// the Rust mirror of the hook's `_canon(url_org) != _canon(expected_org)`
/// decision, extracted so the block/allow logic is unit-testable without
/// spawning the shell script. Empty either side → allowed (parity with the
/// hook's `if declared_org and url_org` guard).
#[must_use]
pub fn pre_push_org_blocked(
    url_org: &str,
    expected_org: &str,
    alias_pairs: &BTreeMap<String, String>,
) -> bool {
    if url_org.is_empty() || expected_org.is_empty() {
        return false;
    }
    let canon = |o: &str| alias_pairs.get(o).cloned().unwrap_or_else(|| o.to_string());
    canon(url_org) != canon(expected_org)
}

/// Load the owner-alias table from the standard hyperforge config dir
/// (`~/.config/hyperforge/config.yaml`). Best-effort: a missing or invalid
/// config yields an empty table (identity comparison), so hook install never
/// fails on config problems.
fn owner_alias_pairs() -> BTreeMap<String, String> {
    let Some(home) = dirs::home_dir() else {
        return BTreeMap::new();
    };
    let cfg_path = home.join(".config").join("hyperforge").join("config.yaml");
    crate::v5::config::load_global(&cfg_path)
        .map(|g| g.owner_alias_pairs())
        .unwrap_or_default()
}

/// Install the pre-push hook to a repo's .hyperforge/hooks/ directory.
/// Renders the hook with the configured owner aliases (HYPE-5).
pub fn install_pre_push_hook(repo_path: &Path, dry_run: bool) -> std::io::Result<bool> {
    let hooks_dir = repo_path.join(".hyperforge").join("hooks");
    let hook_path = hooks_dir.join("pre-push");
    let rendered = render_pre_push_hook(&owner_alias_pairs());

    // Check if hook already exists with same content
    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path)?;
        if existing == rendered {
            return Ok(false); // Already installed, no change
        }
    }

    if dry_run {
        return Ok(true); // Would install
    }

    // Create hooks directory
    fs::create_dir_all(&hooks_dir)?;

    // Write hook
    fs::write(&hook_path, &rendered)?;

    // Make executable (rwxr-xr-x)
    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(&hook_path, perms)?;

    Ok(true) // Installed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias_table() -> BTreeMap<String, String> {
        // hypermemetic (old user) is an alias of the canonical org.
        let mut m = BTreeMap::new();
        m.insert("hypermemetic".to_string(), "hypermemetic-ai".to_string());
        m
    }

    #[test]
    fn prepush_aliased_owner_passes_unrelated_blocks() {
        let aliases = alias_table();
        // Declared org hypermemetic-ai; push URL owner is the alias user.
        // Aliased owner => NOT blocked (the false-block this fixes).
        assert!(!pre_push_org_blocked("hypermemetic", "hypermemetic-ai", &aliases));
        // Same in reverse and when identical.
        assert!(!pre_push_org_blocked("hypermemetic-ai", "hypermemetic", &aliases));
        assert!(!pre_push_org_blocked("hypermemetic-ai", "hypermemetic-ai", &aliases));
        // A genuine unrelated owner still blocks.
        assert!(pre_push_org_blocked("someone-else", "hypermemetic-ai", &aliases));
        // Empty either side => allowed (parity with the hook's guard).
        assert!(!pre_push_org_blocked("", "hypermemetic-ai", &aliases));
        assert!(!pre_push_org_blocked("hypermemetic", "", &aliases));
    }

    #[test]
    fn prepush_without_aliases_is_plain_equality() {
        let empty = BTreeMap::new();
        assert!(pre_push_org_blocked("hypermemetic", "hypermemetic-ai", &empty));
        assert!(!pre_push_org_blocked("hypermemetic-ai", "hypermemetic-ai", &empty));
    }

    #[test]
    fn render_embeds_alias_table_and_canon() {
        let rendered = render_pre_push_hook(&alias_table());
        // The hook carries the alias-aware comparison and the embedded table.
        assert!(rendered.contains("_canon(url_org) != _canon(expected_org)"));
        assert!(rendered.contains("def _canon(o):"));
        assert!(rendered.contains("\\\"hypermemetic\\\": \\\"hypermemetic-ai\\\""));
        assert!(!rendered.contains("__OWNER_ALIASES__"));
    }

    #[test]
    fn render_empty_aliases_is_empty_dict() {
        let rendered = render_pre_push_hook(&BTreeMap::new());
        assert!(rendered.contains("OWNER_ALIASES = {}"));
        assert!(!rendered.contains("__OWNER_ALIASES__"));
    }
}
