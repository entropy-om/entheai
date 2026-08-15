pub mod fs;
pub mod openclaw;
pub mod search;
pub mod shell;
pub mod todo;

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("path escapes workspace root: {0}")]
    PathEscape(String),
    #[error("missing string arg '{0}'")]
    MissingArg(String),
    #[error("command timed out after {secs}s: {command}")]
    Timeout { secs: u64, command: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("tool task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("mcp tool error: {0}")]
    Mcp(String),
    #[error("edit: {0}")]
    Edit(String),
}

/// A callable tool. `schema()` is the OpenAI function-tool JSON schema;
/// `call()` executes with JSON `args` (already parsed) and returns text output.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> serde_json::Value;
    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError>;
    fn tier(&self) -> entheai_permission::Tier {
        entheai_permission::Tier::Exec
    }
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), Arc::from(tool));
    }
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|b| b.as_ref())
    }
    /// All tool schemas, for sending to the model.
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools.values().map(|t| t.schema()).collect()
    }

    /// Owned `Arc` handles to every registered tool, for wrapping (e.g. into
    /// `adk_core::Tool` adapters) without consuming the registry — so the
    /// same registry (and any stateful tools it holds, e.g. MCP-server-backed
    /// ones) can back multiple agent builds, one per turn.
    pub fn to_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.values().cloned().collect()
    }
}

/// Optional overrides for the built-in tools. A `None` field leaves that tool at
/// its own constructor default — so the fan-out orchestrator (which has no
/// `[tools]` limits to thread through) keeps exactly the behaviour it had, while
/// the CLI passes the user's configured limits.
#[derive(Debug, Clone, Default)]
pub struct BuiltinLimits {
    pub shell_timeout_secs: Option<u64>,
    pub shell_output_cap: Option<usize>,
    pub search_max_results: Option<usize>,
}

/// Register the canonical built-in tool set — file `read`/`write`/`edit`,
/// `search`, `run_shell`, and `todo` — rooted at `root`, at their correct
/// permission tiers.
///
/// One source of truth: every layer that builds a write-capable agent (the CLI
/// and the fan-out orchestrator's coders alike) calls this, instead of
/// hand-rolling a list that silently drifts — which is exactly how the fan-out
/// coders had lost the `todo` tool the CLI agent carried.
pub fn register_builtins(reg: &mut ToolRegistry, root: &Path, limits: BuiltinLimits) {
    reg.register(Box::new(fs::ReadFile::new(root.to_path_buf())));
    reg.register(Box::new(fs::WriteFile::new(root.to_path_buf())));
    reg.register(Box::new(fs::EditFile::new(root.to_path_buf())));

    let mut shell = shell::RunShell::new(root.to_path_buf());
    if let (Some(secs), Some(cap)) = (limits.shell_timeout_secs, limits.shell_output_cap) {
        shell = shell.with_limits(secs, cap);
    }
    reg.register(Box::new(shell));

    let mut search = search::Search::new(root.to_path_buf());
    if let Some(n) = limits.search_max_results {
        search = search.with_max_results(n);
    }
    reg.register(Box::new(search));

    reg.register(Box::new(todo::TodoTool));
    reg.register(Box::new(openclaw::OpenClawProbe::default()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ReadFile;
    use std::io::Write;

    #[test]
    fn to_tools_yields_all_registered_without_consuming() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(ReadFile::new(std::env::temp_dir())));
        assert_eq!(reg.to_tools().len(), 1);
        // Registry still usable — to_tools borrows, doesn't consume.
        assert_eq!(reg.to_tools().len(), 1);
    }

    #[test]
    fn register_builtins_yields_the_full_core_set() {
        let mut reg = ToolRegistry::new();
        register_builtins(
            &mut reg,
            std::path::Path::new("/tmp"),
            BuiltinLimits::default(),
        );
        let names: std::collections::HashSet<String> = reg
            .to_tools()
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        // The canonical set every write-capable agent gets — including `todo`,
        // which the fan-out coders used to be missing.
        for expected in [
            "read_file",
            "write_file",
            "edit_file",
            "search",
            "run_shell",
            "todo",
            "openclaw_probe",
        ] {
            assert!(names.contains(expected), "builtin set missing {expected:?}");
        }
        assert_eq!(reg.to_tools().len(), 7, "exactly the seven core builtins");
    }

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
    async fn edit_file_replaces_unique_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nBETA\ngamma").unwrap();
        let tool = crate::fs::EditFile::new(dir.path());
        let out = tool
            .call(serde_json::json!({ "path": "a.txt", "old_str": "BETA", "new_str": "DELTA" }))
            .await
            .unwrap();
        assert!(out.contains("a.txt"));
        let content = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert!(content.contains("DELTA"));
        assert!(!content.contains("BETA"));
    }

    #[tokio::test]
    async fn edit_file_errors_when_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nbeta\ngamma").unwrap();
        let tool = crate::fs::EditFile::new(dir.path());
        let err = tool
            .call(serde_json::json!({ "path": "a.txt", "old_str": "MISSING", "new_str": "x" }))
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn edit_file_errors_when_not_unique() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x\nx\n").unwrap();
        let tool = crate::fs::EditFile::new(dir.path());
        let err = tool
            .call(serde_json::json!({ "path": "a.txt", "old_str": "x", "new_str": "y" }))
            .await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not unique"));
    }

    #[tokio::test]
    async fn edit_file_refuses_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let tool = crate::fs::EditFile::new(dir.path());
        let err = tool
            .call(serde_json::json!({ "path": "../escape.txt", "old_str": "x", "new_str": "y" }))
            .await;
        assert!(err.is_err());
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

    #[test]
    fn builtin_tools_declare_expected_tiers() {
        use entheai_permission::Tier;
        let root = std::path::Path::new("/tmp");
        assert_eq!(ReadFile::new(root).tier(), Tier::Read);
        assert_eq!(crate::fs::WriteFile::new(root).tier(), Tier::Write);
        assert_eq!(crate::fs::EditFile::new(root).tier(), Tier::Write);
        assert_eq!(crate::search::Search::new(root).tier(), Tier::Read);
        assert_eq!(crate::shell::RunShell::new(root).tier(), Tier::Exec);
        assert_eq!(crate::todo::TodoTool.tier(), Tier::Read);
    }
}
