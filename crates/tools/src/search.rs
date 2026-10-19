use async_trait::async_trait;
use std::io::BufRead;
use std::path::PathBuf;
use walkdir::WalkDir;

use crate::fs::MAX_FILE_BYTES;
use crate::{Tool, ToolError};

pub struct Search {
    root: PathBuf,
    max_results: usize,
}
impl Search {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_results: 200,
        }
    }
    /// Override the maximum number of matching lines returned.
    pub fn with_max_results(mut self, n: usize) -> Self {
        self.max_results = n;
        self
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
    fn tier(&self) -> entheai_permission::Tier {
        entheai_permission::Tier::Read
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
        let max_results = self.max_results;
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
                let Ok(file) = std::fs::File::open(entry.path()) else {
                    continue;
                };
                let mut bytes = 0usize;
                // `.lines()` buffers an entire line into memory before the
                // `bytes > MAX_FILE_BYTES` check below ever runs, so a single
                // pathologically long line (minified bundle, generated data)
                // used to allocate unbounded memory regardless of the cap.
                // `Read::take` bounds the reader itself at the IO layer.
                let bounded = std::io::Read::take(file, MAX_FILE_BYTES);
                for (line_no, line) in std::io::BufReader::new(bounded).lines().enumerate() {
                    let Ok(line) = line else { break };
                    bytes += line.len();
                    if bytes > MAX_FILE_BYTES as usize || line.as_bytes().contains(&0) {
                        break;
                    }
                    if line.contains(&query_for_search) {
                        let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                        hits.push(format!(
                            "{}:{}: {}",
                            rel.display(),
                            line_no + 1,
                            line.trim()
                        ));
                        if hits.len() >= max_results {
                            return (hits, true);
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
                "{}\n...[truncated at {max_results} matches]",
                hits.join("\n")
            ))
        } else {
            Ok(hits.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn search_respects_max_results() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "needle here\n").unwrap();
        }
        let uncapped = Search::new(dir.path().to_path_buf());
        let uncapped_out = uncapped
            .call(serde_json::json!({"query": "needle"}))
            .await
            .unwrap();
        assert_eq!(uncapped_out.lines().count(), 5);

        let capped = Search::new(dir.path().to_path_buf()).with_max_results(1);
        let capped_out = capped
            .call(serde_json::json!({"query": "needle"}))
            .await
            .unwrap();
        // 1 matching line + 1 truncation-notice line.
        assert_eq!(capped_out.lines().count(), 2);
        assert!(
            capped_out.contains("truncated at 1 matches"),
            "expected truncation notice: {capped_out}"
        );
    }

    #[tokio::test]
    async fn a_single_pathologically_long_line_does_not_block_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        // One line well past MAX_FILE_BYTES, with no newline at all — the old
        // `.lines()` (no `Read::take`) would buffer the whole thing into one
        // `String` before the byte-count check ever ran.
        let huge_line = "x".repeat(MAX_FILE_BYTES as usize + 1_000_000);
        std::fs::write(dir.path().join("huge.txt"), &huge_line).unwrap();
        std::fs::write(dir.path().join("small.txt"), "needle here\n").unwrap();

        let search = Search::new(dir.path().to_path_buf());
        let out = search
            .call(serde_json::json!({"query": "needle"}))
            .await
            .unwrap();
        assert!(
            out.contains("small.txt"),
            "search must still complete and find the real match: {out}"
        );
    }
}
