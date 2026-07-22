//! I/O layer: gather a pure `RepoContext` from the repo on disk.

use crate::render::{CrateInfo, RenderOptions, RenderOptionsHolder, RepoContext, SourceDoc};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Top-level docs mirrored when present.
const TOP_LEVEL: &[&str] = &["README.md", "AGENTS.md", "CHANGELOG.md", "VERSIONING.md"];

/// Cross-tick read cache. The watcher re-scans the repo on every debounced FS
/// event, but almost every file is unchanged between ticks. Keyed by absolute
/// path → (mtime, len, content); a hit (same mtime AND len) serves the stored
/// content and skips the disk read, so a tick that changed one doc no longer
/// re-reads every doc, every crate manifest/`lib.rs`, and `CLAUDE.md`. Owned by
/// the sync loop and reused across ticks; dropped at session end (so it is
/// bounded by the repo's file count). The mtime+len key is the standard
/// incremental heuristic (cargo/make); rendering is still content-driven, so a
/// hit only ever avoids I/O, never changes output.
#[derive(Default)]
pub struct ScanCache {
    files: HashMap<PathBuf, CachedFile>,
}

struct CachedFile {
    mtime: SystemTime,
    len: u64,
    content: String,
}

impl ScanCache {
    /// Read `path` to a string, serving cached content when the file's
    /// mtime+len are unchanged since the last read. `None` if unreadable
    /// (mirrors `std::fs::read_to_string(..).ok()`).
    fn read(&mut self, path: &Path) -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?;
        let len = meta.len();
        if let Some(cf) = self.files.get(path) {
            if cf.mtime == mtime && cf.len == len {
                return Some(cf.content.clone());
            }
        }
        let content = std::fs::read_to_string(path).ok()?;
        self.files.insert(
            path.to_path_buf(),
            CachedFile {
                mtime,
                len,
                content: content.clone(),
            },
        );
        Some(content)
    }
}

/// Read a repo into a `RepoContext` with a fresh (uncached) read of every file.
/// Convenience for one-shot callers and tests; the live watcher uses
/// [`scan_cached`] with a persistent [`ScanCache`].
pub fn scan(root: &Path, options: RenderOptions) -> io::Result<RepoContext> {
    scan_cached(root, options, &mut ScanCache::default())
}

/// Read a repo into a `RepoContext`, serving unchanged files from `cache`.
/// Missing sources are simply absent (per-source conditional). `options`
/// toggles the architecture/session scans.
pub fn scan_cached(
    root: &Path,
    options: RenderOptions,
    cache: &mut ScanCache,
) -> io::Result<RepoContext> {
    let repo_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());

    let docs = read_markdown_tree(&root.join("docs"), root, cache);
    let top_level = TOP_LEVEL
        .iter()
        .filter_map(|name| read_doc(root, Path::new(name), cache))
        .collect();
    let sessions = if options.include_sessions {
        read_markdown_tree(&root.join(".remember"), root, cache)
    } else {
        Vec::new()
    };
    let crates = if options.include_architecture {
        read_crates(root, cache)
    } else {
        Vec::new()
    };
    let repowise_index = cache
        .read(&root.join("CLAUDE.md"))
        .or_else(|| cache.read(&root.join(".claude").join("CLAUDE.md")));
    let specs = list_markdown(&root.join("docs/superpowers/specs"));
    let plans = list_markdown(&root.join("docs/superpowers/plans"));
    let research = list_markdown(&root.join("docs/research"));
    let root_entries = read_root_entries(root);

    Ok(RepoContext {
        repo_name,
        docs,
        top_level,
        sessions,
        crates,
        root_entries,
        repowise_index,
        specs,
        plans,
        research,
        options: RenderOptionsHolder(options),
    })
}

/// Top-level entry names (files + dirs) for the architecture fallback on
/// non-cargo repos. Skips hidden entries and common build dirs.
fn read_root_entries(root: &Path) -> Vec<String> {
    const SKIP: &[&str] = &["target", "node_modules", "dist"];
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                continue;
            }
            out.push(name);
        }
    }
    out.sort();
    out
}

fn read_doc(root: &Path, rel: &Path, cache: &mut ScanCache) -> Option<SourceDoc> {
    let content = cache.read(&root.join(rel))?;
    Some(SourceDoc {
        repo_rel: rel.to_path_buf(),
        content,
    })
}

/// All `*.md` under `dir`, as `SourceDoc`s with repo-relative paths.
fn read_markdown_tree(dir: &Path, root: &Path, cache: &mut ScanCache) -> Vec<SourceDoc> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("md") {
            if let (Some(content), Ok(rel)) = (cache.read(p), p.strip_prefix(root)) {
                out.push(SourceDoc {
                    repo_rel: rel.to_path_buf(),
                    content,
                });
            }
        }
    }
    out.sort_by(|a, b| a.repo_rel.cmp(&b.repo_rel));
    out
}

/// Repo-relative paths of `*.md` directly informing an index (non-recursive).
fn list_markdown(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Discover workspace crates/bins under `crates/` and `bin/`, using each
/// crate's directory name + its `//!` first line (if any) as the role.
fn read_crates(root: &Path, cache: &mut ScanCache) -> Vec<CrateInfo> {
    let mut out = Vec::new();
    for parent in ["crates", "bin"] {
        let base = root.join(parent);
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten() {
                if e.path().join("Cargo.toml").is_file() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let role = crate_role(&e.path(), cache);
                    out.push(CrateInfo { name, role });
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// First `//!` doc-comment line of `src/lib.rs` or `src/main.rs`, trimmed.
fn crate_role(dir: &Path, cache: &mut ScanCache) -> String {
    for f in ["src/lib.rs", "src/main.rs"] {
        if let Some(text) = cache.read(&dir.join(f)) {
            for line in text.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("//!") {
                    let r = rest.trim();
                    if !r.is_empty() {
                        return r.to_string();
                    }
                }
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_docs_sessions_and_crates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/tui.md"), "# TUI").unwrap();
        std::fs::write(root.join("README.md"), "# proj").unwrap();
        std::fs::create_dir_all(root.join(".remember")).unwrap();
        std::fs::write(root.join(".remember/now.md"), "buffer").unwrap();
        std::fs::create_dir_all(root.join("crates/foo/src")).unwrap();
        std::fs::write(root.join("crates/foo/Cargo.toml"), "").unwrap();
        std::fs::write(root.join("crates/foo/src/lib.rs"), "//! the foo crate\n").unwrap();

        let ctx = scan(root, RenderOptions::default()).unwrap();
        assert_eq!(ctx.docs.len(), 1);
        assert_eq!(ctx.top_level.len(), 1);
        assert_eq!(ctx.sessions.len(), 1);
        assert!(ctx
            .crates
            .iter()
            .any(|c| c.name == "foo" && c.role == "the foo crate"));
        assert!(ctx.root_entries.contains(&"docs".to_string()));
        assert!(ctx.root_entries.contains(&"README.md".to_string()));
        assert!(
            !ctx.root_entries.iter().any(|e| e.starts_with('.')),
            "hidden entries excluded"
        );
    }

    #[test]
    fn bare_dir_scans_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = scan(dir.path(), RenderOptions::default()).unwrap();
        assert!(ctx.docs.is_empty() && ctx.sessions.is_empty() && ctx.crates.is_empty());
    }

    #[test]
    fn scan_cached_picks_up_content_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/a.md"), "# One").unwrap();

        let mut cache = ScanCache::default();
        let ctx1 = scan_cached(root, RenderOptions::default(), &mut cache).unwrap();
        assert_eq!(ctx1.docs[0].content, "# One");

        // Different length → the (mtime,len) key misses regardless of mtime
        // resolution, so the change is re-read from disk.
        std::fs::write(root.join("docs/a.md"), "# One, now much longer").unwrap();
        let ctx2 = scan_cached(root, RenderOptions::default(), &mut cache).unwrap();
        assert_eq!(ctx2.docs[0].content, "# One, now much longer");
    }

    #[test]
    fn scan_cache_serves_stored_content_on_metadata_match() {
        // White-box: when a file's (mtime,len) are unchanged, the stored content
        // is served without touching disk. Pre-seed the cache with different
        // content of the SAME length under the real file's metadata and prove the
        // scan returns the cached copy, not what's on disk.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let p = root.join("docs/a.md");
        std::fs::write(&p, "DISK").unwrap(); // 4 bytes
        let meta = std::fs::metadata(&p).unwrap();

        let mut cache = ScanCache::default();
        cache.files.insert(
            p.clone(),
            CachedFile {
                mtime: meta.modified().unwrap(),
                len: meta.len(),        // matches disk → cache hit
                content: "CASH".into(), // 4 bytes, distinct from disk "DISK"
            },
        );

        let ctx = scan_cached(root, RenderOptions::default(), &mut cache).unwrap();
        assert_eq!(
            ctx.docs[0].content, "CASH",
            "cached content served on a metadata match (no disk re-read)"
        );
    }
}
