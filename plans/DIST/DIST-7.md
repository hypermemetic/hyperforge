# DIST-7: Homebrew Formula Generation

blocked_by: [DIST-4]
unlocks: []

## Scope

Generate Homebrew formula files from release assets so users can `brew install org/tap/package`. Covers both Rust and Haskell binaries — Homebrew doesn't care what language built the binary.

## How Homebrew Taps Work

A tap is a git repo (e.g. `github.com/hypermemetic/homebrew-tap`) containing formula files:

```ruby
class Synapse < Formula
  desc "Schema-driven CLI for Plexus RPC servers"
  homepage "https://github.com/hypermemetic/synapse"
  version "3.10.1"

  on_macos do
    on_arm do
      url "https://github.com/hypermemetic/synapse/releases/download/v3.10.1/synapse-aarch64-apple-darwin-v3.10.1.tar.gz"
      sha256 "abc123..."
    end
    on_intel do
      url "https://github.com/hypermemetic/synapse/releases/download/v3.10.1/synapse-x86_64-apple-darwin-v3.10.1.tar.gz"
      sha256 "def456..."
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/hypermemetic/synapse/releases/download/v3.10.1/synapse-aarch64-unknown-linux-gnu-v3.10.1.tar.gz"
      sha256 "ghi789..."
    end
    on_intel do
      url "https://github.com/hypermemetic/synapse/releases/download/v3.10.1/synapse-x86_64-unknown-linux-gnu-v3.10.1.tar.gz"
      sha256 "jkl012..."
    end
  end

  def install
    bin.install "synapse"
  end
end
```

## Method

`build brew_formula` — generates a formula file from an existing release.

### Params
- `org` — organization name
- `name` — package name
- `tag` — release tag to generate formula from
- `forge` — which forge hosts the release (default: github)
- `tap_path` — path to the homebrew-tap repo (optional, outputs to stdout if not set)
- `description` — formula description (optional, read from Cargo.toml/cabal if available)

### Flow

1. Fetch release by tag via ReleasePort
2. List assets on the release
3. Match asset names to target triples (parse the binstall naming convention)
4. Download each asset, compute sha256
5. Generate Ruby formula mapping targets to Homebrew platform selectors
6. Write to `{tap_path}/Formula/{name}.rb` or emit as output

## Tap Management

A companion method `build brew_tap_init` creates the tap repo structure:

```
homebrew-tap/
  Formula/
    .gitkeep
  README.md
```

And registers it in LocalForge so `hyperforge sync` keeps it pushed.

## Acceptance Criteria

- [ ] Generates valid Homebrew formula from release assets
- [ ] Maps target triples to Homebrew platform selectors (on_macos/on_linux, on_arm/on_intel)
- [ ] Computes sha256 for each asset
- [ ] Handles multi-binary packages (multiple `bin.install` lines)
- [ ] `brew install org/tap/name` works after pushing the formula
