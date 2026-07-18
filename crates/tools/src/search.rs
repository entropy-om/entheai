use async_trait::async_trait;
use std::path::PathBuf;
use walkdir::WalkDir;

use crate::{Tool, ToolError};

pub struct Search {
    root: PathBuf,
}
impl Search {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

fn is_excluded_dir(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir()
        && matches!(
            entry.file_name().to_str(),
            Some(".git" | "target" | "node_modules" | ".venv" | "dist" | "build")
        )
}
#[async_trait]
impl Tool for Search {
    fn name(&self) -> &str {
        "search"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Substring-search text files under the workspace; returns matching file:line: text.",
                "parameters": {
                    "type": "object",
                    "properties": { "query": { "type": "string", "description": "Substring to find." } },
                    "required": ["query"]
                }
            }
        })
    }
    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| ToolError::MissingArg("query".into()))?
            .to_string();
        let query_for_search = query.clone();
        let root = self.root.clone();
        let (hits, truncated) = tokio::task::spawn_blocking(move || {
            let mut hits = Vec::new();
            for entry in WalkDir::new(&root)
                .into_iter()
                .filter_entry(|e| !is_excluded_dir(e))
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    for (i, line) in text.lines().enumerate() {
                        if line.contains(&query_for_search) {
                            let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                            hits.push(format!("{}:{}: {}", rel.display(), i + 1, line.trim()));
                            if hits.len() >= 200 {
                                return (hits, true);
                            }
                        }
                    }
                }
            }
            (hits, false)
        })
        .await?;
        if hits.is_empty() {
            Ok(format!("no matches for {query:?}"))
        } else if truncated {
            Ok(format!(
                "{}\n...[truncated at 200 matches]",
                hits.join("\n")
            ))
        } else {
            Ok(hits.join("\n"))
        }
    }
}
