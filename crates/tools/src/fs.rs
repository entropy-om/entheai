use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::{Tool, ToolError};

/// Resolve `rel` against `root`, rejecting any path that escapes `root`.
fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, ToolError> {
    let joined = root.join(rel);
    // Reject `..` traversal without requiring the file to exist yet (write_file).
    let mut normalized = PathBuf::new();
    for comp in joined.components() {
        use std::path::Component::*;
        match comp {
            ParentDir => {
                if !normalized.starts_with(root) || normalized == *root {
                    return Err(ToolError::PathEscape(rel.to_string()));
                }
                normalized.pop();
            }
            CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    if !normalized.starts_with(root) {
        return Err(ToolError::PathEscape(rel.to_string()));
    }

    // Symlink defense: the lexical `..` check above can't see through symlinks. Canonicalize
    // the deepest EXISTING ancestor of the target and confirm it's still inside `root`. A
    // symlink can only redirect an existing component, so checking the existing prefix is
    // sufficient; not-yet-created files (write_file) fall back to their nearest existing
    // parent (worst case, `root` itself). `root` itself is canonicalized here too — callers
    // may pass it as-is (e.g. macOS temp dirs live under `/var`, itself a symlink to
    // `/private/var`), so comparing against a raw `root` would produce false positives.
    let mut ancestor: &Path = normalized.as_path();
    while !ancestor.exists() {
        match ancestor.parent() {
            Some(p) => ancestor = p,
            None => break,
        }
    }
    if let Ok(canonical) = ancestor.canonicalize() {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if !canonical.starts_with(&canonical_root) {
            return Err(ToolError::PathEscape(rel.to_string()));
        }
    }
    Ok(normalized)
}

fn path_arg(args: &serde_json::Value) -> Result<String, ToolError> {
    args["path"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| ToolError::MissingArg("path".into()))
}

pub struct ReadFile {
    root: PathBuf,
}
impl ReadFile {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}
#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Read a UTF-8 text file within the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Path relative to the workspace root." } },
                    "required": ["path"]
                }
            }
        })
    }
    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let rel = path_arg(&args)?;
        let path = resolve_in_root(&self.root, &rel)?;
        Ok(tokio::fs::read_to_string(&path).await?)
    }
}

pub struct WriteFile {
    root: PathBuf,
}
impl WriteFile {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}
#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Create or overwrite a UTF-8 text file within the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the workspace root." },
                        "content": { "type": "string", "description": "Full file contents to write." }
                    },
                    "required": ["path", "content"]
                }
            }
        })
    }
    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let rel = path_arg(&args)?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| ToolError::MissingArg("content".into()))?;
        let path = resolve_in_root(&self.root, &rel)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(format!("wrote {} bytes to {rel}", content.len()))
    }
}

pub struct EditFile {
    root: PathBuf,
}
impl EditFile {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}
#[async_trait]
impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace an exact, UNIQUE occurrence of old_str with new_str in a file. old_str must match exactly (including whitespace and indentation) and occur exactly once — include enough surrounding context to make it unique. Prefer this over write_file for edits.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path relative to the workspace root"},
                        "old_str": {"type": "string", "description": "Exact text to find (must be unique in the file)"},
                        "new_str": {"type": "string", "description": "Replacement text"}
                    },
                    "required": ["path", "old_str", "new_str"]
                }
            }
        })
    }
    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::MissingArg("path".into()))?;
        let old_str = args["old_str"]
            .as_str()
            .ok_or_else(|| ToolError::MissingArg("old_str".into()))?;
        let new_str = args["new_str"]
            .as_str()
            .ok_or_else(|| ToolError::MissingArg("new_str".into()))?;
        let full = resolve_in_root(&self.root, path)?;
        let content = tokio::fs::read_to_string(&full).await?;
        let count = content.matches(old_str).count();
        if count == 0 {
            return Err(ToolError::Edit(format!("old_str not found in {path}")));
        }
        if count > 1 {
            return Err(ToolError::Edit(format!(
                "old_str is not unique in {path} ({count} matches) — include more surrounding context"
            )));
        }
        let updated = content.replacen(old_str, new_str, 1);
        tokio::fs::write(&full, updated).await?;
        Ok(format!("edited {path}: 1 replacement"))
    }
}
