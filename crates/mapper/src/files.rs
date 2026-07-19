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
#[allow(dead_code)] // consumed by read_and_chunk() in Task 3
pub(crate) const CHUNK_LINES: usize = 200;

/// Extract `@{path}` references from `text`. Returns the text with each
/// `@{path}` token replaced by a short `[file: path]` marker, plus the list of
/// raw path strings found (in order, may contain duplicates).
#[allow(dead_code)] // consumed by Mapper::map() in Task 4
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
#[allow(dead_code)] // consumed by Mapper::map() in Task 4
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
