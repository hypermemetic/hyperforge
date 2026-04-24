# Tier-2 Test Config Template

This directory is a **template**, not a live fixture. To run the v5
tier-2 suite (adapter + sync + push tests against real forges), copy
this tree to a user-owned location, fill in real values, and point
`HF_V5_TEST_CONFIG_DIR` at it.

## Why

Tier-2 tests exercise real GitHub / Codeberg / GitLab APIs. The
configuration the daemon reads during the test is **format-identical
to production** — same `config.yaml`, same `orgs/<org>.yaml`, same
`secrets.yaml`. The only non-production addition is `tier2.env`, which
pins which org/repo the tests should operate on (tests need a
disposable repo; production users do not).

"Pass in the same thing that the actual system reads" = the daemon
sees the same on-disk shape whether it's running in production or
under test.

## Setup

```bash
# 1. Copy the template somewhere outside the repo.
cp -r tests/v5/fixtures/tier2-template ~/.config/hyperforge-v5-test

# 2. Fill in real values (see each file's comments).
$EDITOR ~/.config/hyperforge-v5-test/tier2.env
$EDITOR ~/.config/hyperforge-v5-test/secrets.yaml
$EDITOR ~/.config/hyperforge-v5-test/orgs/example.yaml

# 3. Point the harness at it.
export HF_V5_TEST_CONFIG_DIR=~/.config/hyperforge-v5-test

# 4. Run the tier-2 tests.
cargo test --test v5_integration --features tier2
# or individual scripts:
bash tests/v5/V5REPOS/V5REPOS-9.sh
```

## Required disposable repo

Tests that push metadata (`V5REPOS-14`) update the remote repo's
`description` field and then restore the original. Use a **disposable
repo you control**, not a load-bearing one. Anything named `sandbox-*`
or `test-*` that you don't mind the description being briefly rewritten
on.

## File shapes

### `tier2.env`

Bash-sourceable. Blank values skip that forge's tests.

```bash
# Test target for GitHub adapter / sync / push (V5REPOS-9, -13, -14, V5WS-9)
HF_TIER2_GITHUB_ORG=hypermemetic
HF_TIER2_GITHUB_REPO=sandbox-v5-tier2

# Test target for Codeberg (V5REPOS-10)
HF_TIER2_CODEBERG_ORG=
HF_TIER2_CODEBERG_REPO=

# Test target for GitLab (V5REPOS-11)
HF_TIER2_GITLAB_ORG=
HF_TIER2_GITLAB_REPO=
```

### `config.yaml`

Same shape as production. Provider-domain map:

```yaml
provider_map:
  github.com: github
  codeberg.org: codeberg
  gitlab.com: gitlab
```

### `orgs/<org>.yaml`

One file per org, using `secrets://` refs for credentials (never
inline). The org's `repos:` list should include the test-target repo
with its remote URL.

```yaml
name: hypermemetic
forge:
  provider: github
  credentials:
    - key: secrets://github/hypermemetic/token
      type: token
repos:
  - name: sandbox-v5-tier2
    remotes:
      - url: https://github.com/hypermemetic/sandbox-v5-tier2.git
```

### `secrets.yaml`

Real secret values. **Keep this file outside the repo.** If you copy
the template into `~/.config/hyperforge-v5-test/`, filesystem perms
protect it at the dir level (mode 700 recommended).

```yaml
github/hypermemetic/token: ghp_xxxxxxxxxxxxxxxxxxxx
codeberg/hypermemetic/token: abc123...
gitlab/hypermemetic/token: glpat-...
```

## Suggested source for the GitHub token

If you already use the `gh` CLI:

```bash
gh auth token > /tmp/gh-token
echo "github/hypermemetic/token: $(cat /tmp/gh-token)" >> ~/.config/hyperforge-v5-test/secrets.yaml
rm /tmp/gh-token
```

Scope required: `repo` (for description writes on a repo you own).

## What `HF_V5_TEST_CONFIG_DIR` unset means

Every tier-2 script calls `hf_require_tier2` at the top. When the env
var isn't set, the script prints `SKIP: ...` and exits 0 — tier-1
runs stay green, tier-2 runs stay SKIP-clean. No forced failures.
