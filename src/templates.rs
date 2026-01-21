//! Default templates for new repositories.

/// Default .gitignore content for new repositories.
/// Covers common patterns for Rust, Node.js, Python, and general development.
pub const DEFAULT_GITIGNORE: &str = r#"# Build outputs
/target/
/dist/
/build/
*.o
*.a
*.so
*.dylib

# Dependencies
/node_modules/
/vendor/
/.venv/
/venv/
__pycache__/
*.pyc

# IDE and editor
.idea/
.vscode/
*.swp
*.swo
*~
.DS_Store

# Environment and secrets
.env
.env.local
.env.*.local
*.pem
*.key

# Logs and caches
*.log
.cache/
.pytest_cache/
.mypy_cache/

# Package lock files (uncomment if you want to ignore)
# Cargo.lock
# package-lock.json
# yarn.lock

# OS files
Thumbs.db
"#;

/// Returns the default gitignore content.
pub fn default_gitignore() -> &'static str {
    DEFAULT_GITIGNORE
}
