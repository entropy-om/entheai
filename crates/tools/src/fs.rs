use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::Tool;

/// Resolve `rel` against `root`, rejecting any path that escapes `root`.
fn resolve_in_root(root: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    let joined = root.join(rel);
    // Reject `..` traversal without requiring the file to exist yet (write_file).
    let mut normalized = PathBuf::new();
    for comp in joined.components() {
        use std::path::Component::*;
        match comp {
            ParentDir => {
                if !normalized.starts_with(root) || normalized == *root {
                    anyhow::bail!("path escapes workspace root: {rel}");
                }
                normalized.pop();
            }
            CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    if !normalized.starts_with(root) {
        anyhow::bail!("path escapes workspace root: {rel}");
    }
    Ok(normalized)
}

fn path_arg(args: &serde_json::Value) -> anyhow::Result<String> {
    args["path"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("missing string arg 'path'"))
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
                "name": "read_file",
                "description": "Read a UTF-8 text file within the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Path relative to the workspace root." } },
                    "required": ["path"]
                }
            }
        })
    }
    async fn call(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let rel = path_arg(&args)?;
        let path = resolve_in_root(&self.root, &rel)?;
        Ok(std::fs::read_to_string(&path)?)
    }
}
