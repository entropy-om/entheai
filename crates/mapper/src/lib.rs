//! Structures large prompts/tasks/files into sectioned, chunked input for the
//! orchestrator's fan-out decompose step. Never decides fan-out itself.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

mod files;
mod sections;

pub use files::FileChunk;
pub use sections::PromptSection;

/// Stateless entry point: the crate's sole public operation.
pub struct Mapper;

impl Mapper {
    /// Structure `task` (+ any `@{path}`/bare-path references it contains, plus
    /// any caller-supplied `files`) into a `MappedInput`. Never errors:
    /// unreadable files are skipped, not surfaced as failures.
    pub async fn map(root: &Path, task: &str, files: &[PathBuf]) -> MappedInput {
        let (marked_task, inline_refs) = files::extract_at_refs(task);
        let bare_refs = files::scan_bare_paths(&marked_task);

        let mut candidates: Vec<PathBuf> = files.to_vec();
        candidates.extend(inline_refs.into_iter().map(PathBuf::from));
        candidates.extend(bare_refs.into_iter().map(PathBuf::from));

        let resolved = files::resolve_and_dedupe(root, &candidates).await;
        let mut file_chunks = Vec::new();
        for path in resolved {
            if let Some(mut chunks) = files::read_and_chunk(&path).await {
                file_chunks.append(&mut chunks);
            }
        }

        MappedInput {
            sections: sections::split_sections(&marked_task),
            file_chunks,
        }
    }
}

pub struct MappedInput {
    pub sections: Vec<PromptSection>,
    pub file_chunks: Vec<FileChunk>,
}

impl MappedInput {
    /// Render sections + file chunks into one labeled text block, suitable as
    /// the user message body for the orchestrator's decompose call.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for section in &self.sections {
            match &section.heading {
                Some(h) => {
                    let _ = writeln!(out, "## Section: {h}");
                }
                None => out.push_str("## Section (untitled)\n"),
            }
            out.push_str(&section.body);
            if !section.body.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        for chunk in &self.file_chunks {
            let _ = writeln!(
                out,
                "### File: {} (chunk {}/{})",
                chunk.path.display(),
                chunk.chunk_index + 1,
                chunk.total_chunks
            );
            out.push_str(&chunk.content);
            if !chunk.content.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn map_picks_up_at_file_reference() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "line one\nline two\n").unwrap();

        let mapped = Mapper::map(dir.path(), "# Fix bug\nlook at @{notes.txt}", &[]).await;

        assert_eq!(mapped.sections.len(), 1);
        assert!(mapped.sections[0].body.contains("[file: notes.txt]"));
        assert_eq!(mapped.file_chunks.len(), 1);
        assert_eq!(mapped.file_chunks[0].content, "line one\nline two");
    }

    #[tokio::test]
    async fn map_picks_up_explicit_files_param() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("explicit.txt"), "only line\n").unwrap();

        let mapped = Mapper::map(
            dir.path(),
            "just fix it",
            &[PathBuf::from("explicit.txt")],
        )
        .await;

        assert_eq!(mapped.file_chunks.len(), 1);
        assert_eq!(mapped.file_chunks[0].path, dir.path().join("explicit.txt"));
    }

    #[tokio::test]
    async fn map_skips_missing_reference_without_erroring() {
        let dir = tempdir().unwrap();

        let mapped = Mapper::map(dir.path(), "look at @{missing.txt}", &[]).await;

        assert!(mapped.file_chunks.is_empty());
        assert!(mapped.sections[0].body.contains("[file: missing.txt]"));
    }

    #[tokio::test]
    async fn render_produces_labeled_sections_and_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();

        let mapped = Mapper::map(dir.path(), "# Task\ncheck @{a.txt}", &[]).await;
        let rendered = mapped.render();

        assert!(rendered.contains("## Section: Task"));
        assert!(rendered.contains("[file: a.txt]"));
        assert!(rendered.contains("### File: "));
        assert!(rendered.contains("a.txt (chunk 1/1)"));
        assert!(rendered.contains("hello"));
    }

    #[tokio::test]
    async fn map_picks_up_bare_path_fallback() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("crates/foo")).unwrap();
        std::fs::write(dir.path().join("crates/foo/bar.rs"), "fn bar() {}\n").unwrap();

        let mapped = Mapper::map(dir.path(), "fix crates/foo/bar.rs please", &[]).await;

        assert_eq!(mapped.file_chunks.len(), 1);
        assert_eq!(mapped.file_chunks[0].content, "fn bar() {}");
    }

    #[tokio::test]
    async fn map_and_render_handle_multiple_files_and_sections() {
        let dir = tempdir().unwrap();
        let big_lines: Vec<String> = (0..250).map(|i| format!("line{i}")).collect();
        std::fs::write(dir.path().join("big.txt"), big_lines.join("\n") + "\n").unwrap();
        std::fs::write(dir.path().join("small.txt"), "tiny\n").unwrap();

        let task = "# Requirements\ncheck @{big.txt}\n\n## Constraints\nalso check @{small.txt}\n";
        let mapped = Mapper::map(dir.path(), task, &[]).await;
        let rendered = mapped.render();

        assert_eq!(mapped.sections.len(), 2);
        assert_eq!(mapped.file_chunks.len(), 3); // big.txt -> 2 chunks, small.txt -> 1 chunk
        assert!(rendered.contains("## Section: Requirements"));
        assert!(rendered.contains("## Section: Constraints"));
        assert!(rendered.contains("big.txt (chunk 1/2)"));
        assert!(rendered.contains("big.txt (chunk 2/2)"));
        assert!(rendered.contains("small.txt (chunk 1/1)"));
    }
}
