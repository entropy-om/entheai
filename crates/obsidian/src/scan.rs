//! I/O layer: gather a pure `RepoContext` from the repo on disk.

use crate::render::{CrateInfo, RenderOptions, RenderOptionsHolder, RepoContext, SourceDoc};
use std::io;
use std::path::{Path, PathBuf};

/// Top-level docs mirrored when present.
const TOP_LEVEL: &[&str] = &["README.md", "AGENTS.md", "CHANGELOG.md", "VERSIONING.md"];

/// Read a repo into a `RepoContext`. Missing sources are simply absent
/// (per-source conditional). `options` toggles the architecture/session scans.
pub fn scan(root: &Path, options: RenderOptions) -> io::Result<RepoContext> {
    let repo_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());

    let docs = read_markdown_tree(&root.join("docs"), root);
    let top_level = TOP_LEVEL
        .iter()
        .filter_map(|name| read_doc(root, Path::new(name)))
        .collect();
    let sessions = if options.include_sessions {
        read_markdown_tree(&root.join(".remember"), root)
    } else {
        Vec::new()
    };
    let crates = if options.include_architecture {
        read_crates(root)
    } else {
        Vec::new()
    };
    let repowise_index = std::fs::read_to_string(root.join("CLAUDE.md"))
        .ok()
        .or_else(|| std::fs::read_to_string(root.join(".claude").join("CLAUDE.md")).ok());
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

fn read_doc(root: &Path, rel: &Path) -> Option<SourceDoc> {
    let content = std::fs::read_to_string(root.join(rel)).ok()?;
    Some(SourceDoc {
        repo_rel: rel.to_path_buf(),
        content,
    })
}

/// All `*.md` under `dir`, as `SourceDoc`s with repo-relative paths.
fn read_markdown_tree(dir: &Path, root: &Path) -> Vec<SourceDoc> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("md") {
            if let (Ok(content), Ok(rel)) = (std::fs::read_to_string(p), p.strip_prefix(root)) {
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
fn read_crates(root: &Path) -> Vec<CrateInfo> {
    let mut out = Vec::new();
    for parent in ["crates", "bin"] {
        let base = root.join(parent);
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten() {
                if e.path().join("Cargo.toml").is_file() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let role = crate_role(&e.path());
                    out.push(CrateInfo { name, role });
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// First `//!` doc-comment line of `src/lib.rs` or `src/main.rs`, trimmed.
fn crate_role(dir: &Path) -> String {
    for f in ["src/lib.rs", "src/main.rs"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(f)) {
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
}
