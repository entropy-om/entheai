pub mod fs;
pub mod search;
pub mod shell;

use async_trait::async_trait;
use std::collections::HashMap;

/// A callable tool. `schema()` is the OpenAI function-tool JSON schema;
/// `call()` executes with JSON `args` (already parsed) and returns text output.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> serde_json::Value;
    async fn call(&self, args: serde_json::Value) -> anyhow::Result<String>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|b| b.as_ref())
    }
    /// All tool schemas, for sending to the model.
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools.values().map(|t| t.schema()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ReadFile;
    use std::io::Write;

    #[tokio::test]
    async fn read_file_returns_contents_within_root() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("a.txt")).unwrap();
        write!(f, "hello").unwrap();

        let tool = ReadFile::new(dir.path());
        let out = tool
            .call(serde_json::json!({ "path": "a.txt" }))
            .await
            .unwrap();
        assert_eq!(out, "hello");
        assert_eq!(tool.name(), "read_file");
        assert!(tool.schema()["function"]["name"] == "read_file");
    }

    #[tokio::test]
    async fn read_file_refuses_path_escaping_root() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFile::new(dir.path());
        let err = tool.call(serde_json::json!({ "path": "../secret" })).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn write_file_writes_within_root() {
        let dir = tempfile::tempdir().unwrap();
        let tool = crate::fs::WriteFile::new(dir.path());
        let out = tool
            .call(serde_json::json!({ "path": "out.txt", "content": "data" }))
            .await
            .unwrap();
        assert!(out.contains("out.txt"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("out.txt")).unwrap(),
            "data"
        );
    }

    #[tokio::test]
    async fn write_file_refuses_path_escaping_root() {
        let dir = tempfile::tempdir().unwrap();
        let tool = crate::fs::WriteFile::new(dir.path());
        let err = tool
            .call(serde_json::json!({ "path": "../escaped.txt", "content": "x" }))
            .await;
        assert!(err.is_err());
        assert!(!dir.path().parent().unwrap().join("escaped.txt").exists());
    }

    #[tokio::test]
    async fn read_file_refuses_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "sekret").unwrap();
        // A symlink INSIDE root pointing OUTSIDE root.
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("link.txt"),
        )
        .unwrap();
        let root = dir.path().canonicalize().unwrap(); // CLI passes a canonicalized root
        let tool = crate::fs::ReadFile::new(root);
        let err = tool.call(serde_json::json!({ "path": "link.txt" })).await;
        assert!(
            err.is_err(),
            "reading through an escaping symlink must be rejected"
        );
    }

    #[tokio::test]
    async fn run_shell_captures_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let tool = crate::shell::RunShell::new(dir.path());
        let out = tool
            .call(serde_json::json!({ "command": "echo hello" }))
            .await
            .unwrap();
        assert!(out.contains("hello"));
    }

    #[tokio::test]
    async fn search_finds_matching_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nNEEDLE here\nbeta").unwrap();
        let tool = crate::search::Search::new(dir.path());
        let out = tool
            .call(serde_json::json!({ "query": "NEEDLE" }))
            .await
            .unwrap();
        assert!(out.contains("a.txt"));
        assert!(out.contains("NEEDLE here"));
    }
}
