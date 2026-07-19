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

/// Best-effort scan for bare (non-`@{}`-wrapped) path-like tokens: contains a
/// `/` and a plausible extension, and isn't a URL.
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

/// Resolve `candidates` against `root`, rejecting any that escape `root`
/// (absolute paths, `..` traversal, or symlink redirection), keep only paths
/// that exist as files, and dedupe by canonical identity (not the literal
/// joined string, so `foo.rs` and `./foo.rs` collapse to one entry). Each
/// candidate's existence/containment check runs concurrently.
pub async fn resolve_and_dedupe(root: &std::path::Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let Ok(canonical_root) = tokio::fs::canonicalize(root).await else {
        return Vec::new();
    };

    let handles: Vec<_> = candidates
        .iter()
        .cloned()
        .map(|candidate| {
            let root = root.to_path_buf();
            tokio::spawn(async move {
                let joined = if candidate.is_absolute() {
                    candidate
                } else {
                    root.join(&candidate)
                };
                let canonical = tokio::fs::canonicalize(&joined).await.ok();
                let is_file = tokio::fs::metadata(&joined)
                    .await
                    .map(|m| m.is_file())
                    .unwrap_or(false);
                (canonical, joined, is_file)
            })
        })
        .collect();

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for handle in handles {
        // A panicked resolve task is treated like any other unresolvable
        // candidate: skipped, never aborts the whole map.
        let Ok((Some(canonical), joined, is_file)) = handle.await else {
            continue;
        };
        if !canonical.starts_with(&canonical_root) {
            continue; // escapes root -- reject, same as every other read path in this codebase
        }
        if !seen.insert(canonical) {
            continue;
        }
        if is_file {
            out.push(joined);
        }
    }
    out
}

/// Read `path` and split into `CHUNK_LINES`-line chunks. Returns `None` for
/// unreadable or non-UTF8 (binary) files -- a bad reference never aborts the
/// map. Returns `Some(vec![])` for a readable-but-empty file.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

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
    async fn resolve_and_dedupe_dedupes_equivalent_relative_forms() {
        let dir = tempdir().unwrap();
        let existing = dir.path().join("bar.rs");
        std::fs::write(&existing, "fn bar() {}\n").unwrap();

        let candidates = vec![PathBuf::from("bar.rs"), PathBuf::from("./bar.rs")];
        let resolved = resolve_and_dedupe(dir.path(), &candidates).await;
        assert_eq!(
            resolved.len(),
            1,
            "same file via two spellings must dedupe to one entry"
        );
    }

    #[tokio::test]
    async fn resolve_and_dedupe_rejects_paths_escaping_root() {
        let base = tempdir().unwrap();
        let root = base.path().join("root");
        let secret_dir = base.path().join("secret");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&secret_dir).unwrap();
        let secret_file = secret_dir.join("secret.txt");
        std::fs::write(&secret_file, "top secret\n").unwrap();

        let candidates = vec![
            PathBuf::from("../secret/secret.txt"), // relative traversal escape
            secret_file.clone(),                   // absolute path escape
        ];
        let resolved = resolve_and_dedupe(&root, &candidates).await;
        assert!(
            resolved.is_empty(),
            "paths escaping root must never be resolved, even if they exist"
        );
    }

    #[tokio::test]
    async fn resolve_and_dedupe_allows_in_root_files_alongside_rejections() {
        let base = tempdir().unwrap();
        let root = base.path().join("root");
        let secret_dir = base.path().join("secret");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&secret_dir).unwrap();
        let in_root = root.join("real.rs");
        std::fs::write(&in_root, "fn main() {}\n").unwrap();
        std::fs::write(secret_dir.join("secret.txt"), "top secret\n").unwrap();

        let candidates = vec![
            PathBuf::from("real.rs"),
            PathBuf::from("../secret/secret.txt"),
        ];
        let resolved = resolve_and_dedupe(&root, &candidates).await;
        assert_eq!(resolved, vec![in_root]);
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
}
