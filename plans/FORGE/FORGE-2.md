# FORGE-2: Repo struct — replace `origin + mirrors` with `forges`

blocked_by: []
unlocks: [FORGE-4, FORGE-5]

## Scope

Replace `origin: Forge` + `mirrors: Vec<Forge>` with `forges: Vec<Forge>` in the `Repo` struct.

## File

`src/types/repo.rs`

## Changes

### Struct fields

```rust
pub struct Repo {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Visibility,
    pub forges: Vec<Forge>,        // was: origin + mirrors
    pub protected: bool,
    pub aliases: Vec<String>,
}
```

### Constructor

```rust
pub fn new(name: impl Into<String>, forges: Vec<Forge>) -> Self {
    Self {
        name: name.into(),
        description: None,
        visibility: Visibility::Public,
        forges,
        protected: false,
        aliases: Vec::new(),
    }
}
```

### Builder methods

- **Remove**: `with_mirror()`, `with_mirrors()`, `all_forges()`
- **Add**: `with_forge(forge: Forge)` — appends if not already present
- **Add**: `with_forges(forges: Vec<Forge>)` — sets the list, deduplicating
- **Add**: `merge_forge(&mut self, forge: Forge)` — appends if not present (for import merge)

### Serde backward compat

Custom `Deserialize` using untagged helper enum:

```rust
#[derive(Deserialize)]
#[serde(untagged)]
enum RepoHelper {
    New {
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        visibility: Visibility,
        forges: Vec<Forge>,
        #[serde(default)]
        protected: bool,
        #[serde(default)]
        aliases: Vec<String>,
    },
    Old {
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        visibility: Visibility,
        origin: Forge,
        #[serde(default)]
        mirrors: Vec<Forge>,
        #[serde(default)]
        protected: bool,
        #[serde(default)]
        aliases: Vec<String>,
    },
}
```

Old format (`origin` + `mirrors`) deserializes into `forges = [origin] + mirrors`. New format uses `forges` directly. Serialization always writes `forges`. One-way migration — first save converts old to new.

### Acceptance criteria

- `Repo::new("test", vec![Forge::GitHub])` works
- `with_forge()` deduplicates
- `merge_forge()` appends only if new
- Old YAML with `origin: github` + `mirrors: [codeberg]` deserializes to `forges: [github, codeberg]`
- New YAML with `forges: [github, codeberg]` deserializes correctly
- Serialization always writes `forges` (never `origin`/`mirrors`)
