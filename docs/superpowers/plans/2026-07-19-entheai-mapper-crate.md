# entheai-mapper Crate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new `entheai-mapper` crate that structures a raw task string (plus any files it references, via `@{path}` tags or bare paths) into sectioned, chunked input, and wire it into the orchestrator's fan-out decompose step.

**Architecture:** A stateless `Mapper::map(root, task, files) -> MappedInput` async function splits the task text into markdown-derived `PromptSection`s, discovers referenced files (explicit list + `@{path}` tags + bare-path fallback), reads and line-chunks them into `FileChunk`s, and exposes `MappedInput::render()` to produce the text block that replaces the raw task string in `crates/orchestrator`'s `decompose_messages` call. Spec: `docs/superpowers/specs/2026-07-19-entheai-mapper-crate-design.md`.

**Tech Stack:** Rust, `tokio::fs` (async file I/O), `tempfile` (dev-dependency for tests). No `regex` — path/`@{}` detection is hand-rolled string scanning.

---

## File Structure

- Create: `crates/mapper/Cargo.toml` — new crate manifest.
- Create: `crates/mapper/src/lib.rs` — `Mapper`, `MappedInput`, orchestration + `render()`.
- Create: `crates/mapper/src/sections.rs` — `PromptSection` + markdown-aware `split_sections`.
- Create: `crates/mapper/src/files.rs` — `FileChunk` + `@{}`/bare-path discovery + reading/chunking.
- Modify: `Cargo.toml` (workspace root) — add `crates/mapper` to `members`.
- Modify: `crates/orchestrator/Cargo.toml` — add `entheai-mapper` dependency.
- Modify: `crates/orchestrator/src/lib.rs` — `run_fanout_readonly` (~line 225) and `run_fanout` (~line 423) call `Mapper::map` before `decompose_messages`.
- Modify: `crates/tui/src/lib.rs` — add a passthrough test near the existing submit tests (~line 1480).

---

### Task 1: Crate scaffold + `PromptSection` + `split_sections`

**Files:**
- Create: `crates/mapper/Cargo.toml`
- Create: `crates/mapper/src/lib.rs`
- Create: `crates/mapper/src/sections.rs`
- Modify: `Cargo.toml:2` (workspace members)

- [ ] **Step 1: Add the crate to the workspace**

Edit workspace `Cargo.toml`, in the `members` list, insert `"crates/mapper"` right after `"crates/orchestrator"`:

```toml
members = ["crates/config", "crates/providers", "crates/core", "crates/tools", "crates/permission", "crates/tui", "crates/memory", "crates/radio", "crates/companion", "crates/router", "crates/orchestrator", "crates/mapper", "crates/mcp", "crates/skills", "crates/viz", "bin/entheai", "bin/entheai-worker"]
```

- [ ] **Step 2: Write the crate manifest**

Create `crates/mapper/Cargo.toml`:

```toml
[package]
name = "entheai-mapper"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
tokio = { workspace = true, features = ["fs"] }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "fs"] }
tempfile = "3"
```

- [ ] **Step 3: Write the failing test for `split_sections`**

Create `crates/mapper/src/sections.rs`:

```rust
/// One markdown-derived section of a prompt: `#`/`##` heading (if any) + body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSection {
    pub heading: Option<String>,
    pub body: String,
}

/// Split `task` into sections on `#`/`##` markdown headings. Lines before the
/// first heading (or the whole text, if no heading is found) become a single
/// section with `heading: None`. List lines (`-`, `*`, `+`, `1.`) are left as
/// part of whichever section's body they fall in — sectioning only reacts to
/// headings.
pub fn split_sections(task: &str) -> Vec<PromptSection> {
    unimplemented!()
}

/// `# Heading` or `## Heading` -> `Some("Heading")`; anything else -> `None`.
fn heading_text(line: &str) -> Option<String> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_becomes_single_untitled_section() {
        let sections = split_sections("just a plain task, do the thing");
        assert_eq!(
            sections,
            vec![PromptSection {
                heading: None,
                body: "just a plain task, do the thing".to_string(),
            }]
        );
    }

    #[test]
    fn headings_split_into_named_sections() {
        let task = "# Requirements\nDo X\nDo Y\n\n## Constraints\nNo Z\n";
        let sections = split_sections(task);
        assert_eq!(
            sections,
            vec![
                PromptSection {
                    heading: Some("Requirements".to_string()),
                    body: "Do X\nDo Y".to_string(),
                },
                PromptSection {
                    heading: Some("Constraints".to_string()),
                    body: "No Z".to_string(),
                },
            ]
        );
    }

    #[test]
    fn list_lines_stay_inside_their_section_body() {
        let task = "# Steps\n- do X\n- do Y\n1. then Z\n";
        let sections = split_sections(task);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading.as_deref(), Some("Steps"));
        assert_eq!(sections[0].body, "- do X\n- do Y\n1. then Z");
    }

    #[test]
    fn empty_input_yields_one_empty_untitled_section() {
        let sections = split_sections("");
        assert_eq!(
            sections,
            vec![PromptSection {
                heading: None,
                body: String::new(),
            }]
        );
    }
}
```

- [ ] **Step 4: Write a minimal `lib.rs` that compiles the module**

Create `crates/mapper/src/lib.rs`:

```rust
//! Structures large prompts/tasks/files into sectioned, chunked input for the
//! orchestrator's fan-out decompose step. Never decides fan-out itself.

mod sections;

pub use sections::PromptSection;
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `cargo test -p entheai-mapper`
Expected: FAIL — `unimplemented!()` panics in `split_sections`/`heading_text`.

- [ ] **Step 6: Implement `split_sections`/`heading_text`**

Replace the two `unimplemented!()` functions in `crates/mapper/src/sections.rs`:

```rust
pub fn split_sections(task: &str) -> Vec<PromptSection> {
    let mut sections = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_body = String::new();

    for line in task.lines() {
        if let Some(heading) = heading_text(line) {
            if !current_body.trim().is_empty() || current_heading.is_some() {
                sections.push(PromptSection {
                    heading: current_heading.take(),
                    body: current_body.trim_end().to_string(),
                });
            }
            current_heading = Some(heading);
            current_body = String::new();
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }

    if !current_body.trim().is_empty() || current_heading.is_some() {
        sections.push(PromptSection {
            heading: current_heading,
            body: current_body.trim_end().to_string(),
        });
    }

    if sections.is_empty() {
        sections.push(PromptSection {
            heading: None,
            body: task.trim_end().to_string(),
        });
    }

    sections
}

fn heading_text(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    for prefix in ["## ", "# "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest.trim().to_string());
        }
    }
    None
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p entheai-mapper`
Expected: PASS (4 tests: `plain_text_becomes_single_untitled_section`,
`headings_split_into_named_sections`, `list_lines_stay_inside_their_section_body`,
`empty_input_yields_one_empty_untitled_section`).

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/mapper/Cargo.toml crates/mapper/src/lib.rs crates/mapper/src/sections.rs
git commit -m "feat(mapper): scaffold entheai-mapper crate with markdown-aware section splitting"
```

---

### Task 2: `FileChunk` + `@{path}` extraction + bare-path scanning

**Files:**
- Create: `crates/mapper/src/files.rs`
- Modify: `crates/mapper/src/lib.rs`

- [ ] **Step 1: Write the failing tests for `extract_at_refs` and `scan_bare_paths`**

Create `crates/mapper/src/files.rs`:

```rust
use std::path::PathBuf;

/// One size-bounded line-chunk of a file's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChunk {
    pub path: PathBuf,
    pub chunk_index: usize,
    pub total_chunks: usize,
    pub content: String,
}

/// Lines per chunk (see design spec §4.3).
pub(crate) const CHUNK_LINES: usize = 200;

/// Extract `@{path}` references from `text`. Returns the text with each
/// `@{path}` token replaced by a short `[file: path]` marker, plus the list of
/// raw path strings found (in order, may contain duplicates).
pub fn extract_at_refs(text: &str) -> (String, Vec<String>) {
    unimplemented!()
}

/// Best-effort scan for bare (non-`@{}`-wrapped) path-like tokens: contains a
/// `/` and a plausible extension, and isn't a URL.
pub fn scan_bare_paths(text: &str) -> Vec<String> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_single_ref_replaces_with_marker() {
        let (text, refs) = extract_at_refs("please review @{crates/foo/bar.rs} for bugs");
        assert_eq!(text, "please review [file: crates/foo/bar.rs] for bugs");
        assert_eq!(refs, vec!["crates/foo/bar.rs".to_string()]);
    }

    #[test]
    fn extract_multiple_refs() {
        let (text, refs) = extract_at_refs("@{a.rs} and @{b.rs}");
        assert_eq!(text, "[file: a.rs] and [file: b.rs]");
        assert_eq!(refs, vec!["a.rs".to_string(), "b.rs".to_string()]);
    }

    #[test]
    fn extract_no_ref_leaves_text_unchanged() {
        let (text, refs) = extract_at_refs("no references here");
        assert_eq!(text, "no references here");
        assert!(refs.is_empty());
    }

    #[test]
    fn extract_unterminated_ref_emits_literally() {
        let (text, refs) = extract_at_refs("look at @{unterminated");
        assert_eq!(text, "look at @{unterminated");
        assert!(refs.is_empty());
    }

    #[test]
    fn scan_bare_paths_finds_path() {
        assert_eq!(
            scan_bare_paths("check crates/foo/bar.rs please"),
            vec!["crates/foo/bar.rs".to_string()]
        );
    }

    #[test]
    fn scan_bare_paths_ignores_urls() {
        assert!(scan_bare_paths("see https://example.com/a.rs here").is_empty());
    }

    #[test]
    fn scan_bare_paths_strips_trailing_punctuation() {
        assert_eq!(
            scan_bare_paths("look at crates/foo/bar.rs."),
            vec!["crates/foo/bar.rs".to_string()]
        );
    }

    #[test]
    fn scan_bare_paths_ignores_extensionless_tokens() {
        assert!(scan_bare_paths("open a/b/c directory").is_empty());
    }
}
```

- [ ] **Step 2: Wire the new module into `lib.rs`**

Edit `crates/mapper/src/lib.rs`:

```rust
//! Structures large prompts/tasks/files into sectioned, chunked input for the
//! orchestrator's fan-out decompose step. Never decides fan-out itself.

mod files;
mod sections;

pub use files::FileChunk;
pub use sections::PromptSection;
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p entheai-mapper`
Expected: FAIL — `unimplemented!()` panics in `extract_at_refs`/`scan_bare_paths`.

- [ ] **Step 4: Implement `extract_at_refs`/`scan_bare_paths`**

Replace the two `unimplemented!()` functions in `crates/mapper/src/files.rs`:

```rust
pub fn extract_at_refs(text: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut refs = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find("@{") {
        let before = &rest[..start];
        let after_marker = &rest[start..];
        out.push_str(before);
        let after_open = &after_marker[2..]; // skip "@{"
        match after_open.find('}') {
            Some(end) => {
                let path = &after_open[..end];
                out.push_str(&format!("[file: {path}]"));
                refs.push(path.to_string());
                rest = &after_open[end + 1..];
            }
            None => {
                // Unterminated "@{" -- emit it literally and stop scanning.
                out.push_str(after_marker);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    (out, refs)
}

pub fn scan_bare_paths(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|tok| {
            let trimmed =
                tok.trim_matches(|c: char| matches!(c, ',' | '.' | ':' | ';' | ')' | '('));
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                return None;
            }
            if !trimmed.contains('/') {
                return None;
            }
            let last_segment = trimmed.rsplit('/').next().unwrap_or("");
            let has_extension = last_segment
                .rsplit_once('.')
                .map(|(name, ext)| {
                    !name.is_empty()
                        && (1..=6).contains(&ext.len())
                        && ext.chars().all(|c| c.is_ascii_alphanumeric())
                })
                .unwrap_or(false);
            has_extension.then(|| trimmed.to_string())
        })
        .collect()
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p entheai-mapper`
Expected: PASS (12 tests total: the 4 from Task 1 + these 8).

- [ ] **Step 6: Commit**

```bash
git add crates/mapper/src/lib.rs crates/mapper/src/files.rs
git commit -m "feat(mapper): @{path} extraction and bare-path fallback scanning"
```

---

### Task 3: File resolution, deduping, and chunking

**Files:**
- Modify: `crates/mapper/src/files.rs`

- [ ] **Step 1: Write the failing tests for `resolve_and_dedupe` and `read_and_chunk`**

Append to `crates/mapper/src/files.rs`, above the `#[cfg(test)]` line, the new
function signatures:

```rust
/// Resolve `candidates` against `root`, keep only paths that exist as files,
/// and dedupe (by resolved path).
pub async fn resolve_and_dedupe(root: &std::path::Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    unimplemented!()
}

/// Read `path` and split into `CHUNK_LINES`-line chunks. Returns `None` for
/// unreadable or non-UTF8 (binary) files -- a bad reference never aborts the
/// map. Returns `Some(vec![])` for a readable-but-empty file.
pub async fn read_and_chunk(path: &std::path::Path) -> Option<Vec<FileChunk>> {
    unimplemented!()
}
```

Add these tests inside the existing `mod tests` block (add `use std::io::Write;`
and `use tempfile::tempdir;` to its imports, alongside the existing `use super::*;`):

```rust
    #[tokio::test]
    async fn resolve_and_dedupe_filters_missing_and_dedupes() {
        let dir = tempdir().unwrap();
        let existing = dir.path().join("real.rs");
        std::fs::write(&existing, "fn main() {}\n").unwrap();

        let candidates = vec![
            PathBuf::from("real.rs"),
            PathBuf::from("real.rs"), // duplicate
            PathBuf::from("missing.rs"),
        ];
        let resolved = resolve_and_dedupe(dir.path(), &candidates).await;
        assert_eq!(resolved, vec![existing]);
    }

    #[tokio::test]
    async fn read_and_chunk_exact_multiple() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("exact.txt");
        let lines: Vec<String> = (0..400).map(|i| format!("line{i}")).collect();
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let chunks = read_and_chunk(&path).await.unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[0].total_chunks, 2);
        assert!(chunks[0].content.starts_with("line0"));
        assert!(chunks[0].content.ends_with("line199"));
        assert!(chunks[1].content.starts_with("line200"));
        assert!(chunks[1].content.ends_with("line399"));
    }

    #[tokio::test]
    async fn read_and_chunk_with_remainder() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("remainder.txt");
        let lines: Vec<String> = (0..250).map(|i| format!("line{i}")).collect();
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let chunks = read_and_chunk(&path).await.unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].content.lines().count(), 200);
        assert_eq!(chunks[1].content.lines().count(), 50);
        assert_eq!(chunks[1].total_chunks, 2);
    }

    #[tokio::test]
    async fn read_and_chunk_empty_file_returns_no_chunks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        std::fs::write(&path, "").unwrap();

        let chunks = read_and_chunk(&path).await.unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn read_and_chunk_missing_file_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("does-not-exist.txt");

        assert!(read_and_chunk(&path).await.is_none());
    }

    #[tokio::test]
    async fn read_and_chunk_binary_file_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("binary.bin");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&[0xFF, 0xFE, 0x00, 0xFF]).unwrap();

        assert!(read_and_chunk(&path).await.is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p entheai-mapper`
Expected: FAIL to compile/run — `unimplemented!()` in `resolve_and_dedupe`/`read_and_chunk`.

- [ ] **Step 3: Implement `resolve_and_dedupe`/`read_and_chunk`**

Replace the two `unimplemented!()` functions:

```rust
pub async fn resolve_and_dedupe(root: &std::path::Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let resolved = if candidate.is_absolute() {
            candidate.clone()
        } else {
            root.join(candidate)
        };
        if !seen.insert(resolved.clone()) {
            continue;
        }
        if tokio::fs::metadata(&resolved)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            out.push(resolved);
        }
    }
    out
}

pub async fn read_and_chunk(path: &std::path::Path) -> Option<Vec<FileChunk>> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let content = String::from_utf8(bytes).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Some(Vec::new());
    }
    let chunks: Vec<&[&str]> = lines.chunks(CHUNK_LINES).collect();
    let total_chunks = chunks.len();
    Some(
        chunks
            .into_iter()
            .enumerate()
            .map(|(i, lines)| FileChunk {
                path: path.to_path_buf(),
                chunk_index: i,
                total_chunks,
                content: lines.join("\n"),
            })
            .collect(),
    )
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p entheai-mapper`
Expected: PASS (18 tests total).

- [ ] **Step 5: Commit**

```bash
git add crates/mapper/src/files.rs
git commit -m "feat(mapper): resolve/dedupe file candidates and line-chunk their content"
```

---

### Task 4: `Mapper::map` + `MappedInput::render`

**Files:**
- Modify: `crates/mapper/src/lib.rs`

- [ ] **Step 1: Write the failing tests for `Mapper::map` and `render`**

Replace `crates/mapper/src/lib.rs` entirely:

```rust
//! Structures large prompts/tasks/files into sectioned, chunked input for the
//! orchestrator's fan-out decompose step. Never decides fan-out itself.

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
        unimplemented!()
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
        unimplemented!()
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
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p entheai-mapper`
Expected: FAIL — `unimplemented!()` panics in `Mapper::map`/`MappedInput::render`.

- [ ] **Step 3: Implement `Mapper::map` and `MappedInput::render`**

Replace the two `unimplemented!()` bodies in `crates/mapper/src/lib.rs`:

```rust
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
```

```rust
    pub fn render(&self) -> String {
        let mut out = String::new();
        for section in &self.sections {
            match &section.heading {
                Some(h) => out.push_str(&format!("## Section: {h}\n")),
                None => out.push_str("## Section (untitled)\n"),
            }
            out.push_str(&section.body);
            if !section.body.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        for chunk in &self.file_chunks {
            out.push_str(&format!(
                "### File: {} (chunk {}/{})\n",
                chunk.path.display(),
                chunk.chunk_index + 1,
                chunk.total_chunks
            ));
            out.push_str(&chunk.content);
            if !chunk.content.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out
    }
```

(Keep the `Mapper` and `MappedInput` type/impl block declarations that already
wrap these bodies — only the two `unimplemented!()` lines are replaced.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p entheai-mapper`
Expected: PASS (22 tests total).

- [ ] **Step 5: Commit**

```bash
git add crates/mapper/src/lib.rs
git commit -m "feat(mapper): Mapper::map orchestration and MappedInput::render"
```

---

### Task 5: Wire the mapper into `entheai-orchestrator`

**Files:**
- Modify: `crates/orchestrator/Cargo.toml`
- Modify: `crates/orchestrator/src/lib.rs:225` (`run_fanout_readonly`)
- Modify: `crates/orchestrator/src/lib.rs:423` (`run_fanout`)

- [ ] **Step 1: Add the dependency**

Edit `crates/orchestrator/Cargo.toml`, in `[dependencies]`, add:

```toml
entheai-mapper = { path = "../mapper" }
```

- [ ] **Step 2: Write the contract test that `decompose_messages` accepts mapped output**

This test locks in the contract the wiring edit in Steps 3-4 relies on: that
`decompose_messages` fed with `Mapper::map(...).render()` carries structured
markers instead of the raw task string. It exercises already-implemented
code (`Mapper::map` from Task 4, `decompose_messages` already existing), so
it passes as soon as it's written — there's no red step here, since no new
production code is introduced by this test alone. Add to the `#[cfg(test)]
mod tests` block in `crates/orchestrator/src/lib.rs` (near the other tests,
e.g. after `decomposed_carries_tasks`). `crates/orchestrator/Cargo.toml`
already has `tempfile = "3"` in `[dev-dependencies]`:

```rust
    #[tokio::test]
    async fn decompose_input_is_mapped_not_raw_task() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("notes.txt"), "line one\nline two\n")
            .await
            .unwrap();
        let task = "# Fix bug\nlook at @{notes.txt}";

        let mapped = entheai_mapper::Mapper::map(dir.path(), task, &[]).await;
        let messages = decompose_messages(&mapped.render());

        let user_msg = messages
            .iter()
            .find(|m| m.role == "user")
            .expect("user message");
        assert!(user_msg.content.contains("## Section: Fix bug"));
        assert!(user_msg.content.contains("[file: notes.txt]"));
        assert!(user_msg.content.contains("### File: "));
        assert!(user_msg.content.contains("line one"));
        assert_ne!(user_msg.content, task);
    }
```

- [ ] **Step 3: Run the contract test to verify it passes**

Run: `cargo test -p entheai-orchestrator decompose_input_is_mapped_not_raw_task`
Expected: PASS. This confirms the contract; Steps 4-5 now wire `run_fanout`/
`run_fanout_readonly` to actually rely on it (verified by the full-suite run
in Step 6 and by re-reading the two call sites match this pattern exactly).

- [ ] **Step 4: Wire `Mapper::map` into `run_fanout_readonly`**

In `crates/orchestrator/src/lib.rs`, replace the body of `run_fanout_readonly`
(currently starting around line 225):

```rust
async fn run_fanout_readonly(config: &Config, root: &Path, task: &str) -> anyhow::Result<String> {
    let orch_model = entheai_router::orchestrator_model(config)?;

    // 1. Map + decompose.
    let mapped = entheai_mapper::Mapper::map(root, task, &[]).await;
    let raw = orchestrate_once(config, &orch_model, decompose_messages(&mapped.render())).await?;
    let max_par = config.router.max_parallel.max(1);
    let subtasks = parse_decomposition(&raw, max_par);

    // Fallback: couldn't decompose → just run the task once on the orchestrator.
    if subtasks.is_empty() {
        return orchestrate_once(config, &orch_model, vec![ChatMessage::user(task)]).await;
    }

    // 2. Fan out, bounded by max_parallel.
    let results: Vec<SubResult> = stream::iter(subtasks)
        .map(|st| run_subagent(config, root, st))
        .buffer_unordered(max_par)
        .collect()
        .await;

    // 3. Synthesize.
    orchestrate_once(config, &orch_model, synthesis_messages(task, &results)).await
}
```

(Only the addition of the `Mapper::map` call and the `decompose_messages`
argument changed — the fallback still uses the raw `task`, and synthesis is
untouched.)

- [ ] **Step 5: Wire `Mapper::map` into `run_fanout`**

In `crates/orchestrator/src/lib.rs`, in `run_fanout` (currently starting
around line 423), replace:

```rust
    let orch_model = entheai_router::orchestrator_model(config)?;
    let max_par = config.router.max_parallel.max(1);

    // 1. Decompose.
    let raw = orchestrate_once(config, &orch_model, decompose_messages(task)).await?;
    let subtasks = parse_decomposition(&raw, max_par);
```

with:

```rust
    let orch_model = entheai_router::orchestrator_model(config)?;
    let max_par = config.router.max_parallel.max(1);

    // 1. Map + decompose.
    let mapped = entheai_mapper::Mapper::map(root, task, &[]).await;
    let raw = orchestrate_once(config, &orch_model, decompose_messages(&mapped.render())).await?;
    let subtasks = parse_decomposition(&raw, max_par);
```

(Everything after this — the empty-subtasks fallback using raw `task`, the
worktree creation, coder dispatch, and the final report — is unchanged.)

- [ ] **Step 6: Run the full orchestrator test suite**

Run: `cargo test -p entheai-orchestrator`
Expected: PASS, including `decompose_input_is_mapped_not_raw_task`.

- [ ] **Step 7: Build the whole workspace to catch any missed call sites**

Run: `cargo build --workspace`
Expected: builds clean (no errors from `bin/entheai` or `crates/tui` call
sites — `run_fanout`'s public signature is unchanged, only its internals
changed).

- [ ] **Step 8: Commit**

```bash
git add crates/orchestrator/Cargo.toml crates/orchestrator/src/lib.rs
git commit -m "feat(orchestrator): route task text through entheai-mapper before decompose"
```

---

### Task 6: TUI `@{file}` passthrough test

**Files:**
- Modify: `crates/tui/src/lib.rs` (test module, near line 1480)

- [ ] **Step 1: Write the passthrough test**

Add to the `#[cfg(test)] mod tests` block in `crates/tui/src/lib.rs`, right
after `plain_message_does_not_submit_while_working`:

```rust
    #[test]
    fn at_file_reference_survives_submit_unmodified() {
        let mut app = App {
            messages: Vec::new(),
            input: "@{crates/tui/src/lib.rs} fix the input handler".to_string(),
            status: Status::Idle,
            scroll: 0,
            follow: true,
            model_label: "m".into(),
            pending_permission: None,
            run_started: None,
            spinner_frame: 0,
            current_action: String::new(),
            now_playing: None,
            fanout: true,
            worker_pool: None,
            system_prompt: None,
            streaming_idx: None,
            out_tokens: 0,
            verb_idx: 0,
            plan: Vec::new(),
            swarm: entheai_viz::SwarmModel::new(),
            view: ViewMode::Chat,
            viz_swarm: false,
        };
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let action = handle_key(&mut app, key);
        match action {
            Action::Submit(text) => {
                assert_eq!(text, "@{crates/tui/src/lib.rs} fix the input handler")
            }
            _ => panic!("expected Action::Submit for an idle message containing @{{file}}"),
        }
    }
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p entheai-tui at_file_reference_survives_submit_unmodified`
Expected: PASS — no TUI code changes were needed, since `KeyCode::Char`
already pushes any character (including `@`, `{`, `}`) into `app.input`
unmodified, and `Action::Submit(text)` carries it verbatim.

- [ ] **Step 3: Run the full TUI test suite to confirm no regressions**

Run: `cargo test -p entheai-tui`
Expected: PASS (all existing tests plus the new one).

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/lib.rs
git commit -m "test(tui): verify @{file} references survive input submit unmodified"
```

---

## Final Verification

- [ ] Run `cargo test --workspace` — everything green.
- [ ] Run `cargo build --workspace` — clean build.
- [ ] Skim `docs/superpowers/specs/2026-07-19-entheai-mapper-crate-design.md`
  against the six tasks above: §3 (API) → Tasks 1/2/3/4; §4.1 (prompt
  structuring) → Task 1; §4.2 (file discovery) → Task 2; §4.3 (chunking) →
  Task 3; §4.4 (rendering) → Task 4; §5 (TUI trigger) → Task 6; §6
  (integration) → Task 5.
