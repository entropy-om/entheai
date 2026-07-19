//! Pure per-note generators. Each is per-source conditional: a missing source
//! contributes nothing (never an error).

use crate::render::{front_matter, AssetRef, RenderOutput, RepoContext, SourceDoc, VaultNote};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// `![alt](target)` or `[text](target)`, capturing the leading `!`, the text,
/// and the target (no support for reference-style links — YAGNI for v1).
fn link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(!?)\[([^\]]*)\]\(([^)\s]+)\)").unwrap())
}

/// True for external/anchor targets we must never rewrite.
fn is_external(target: &str) -> bool {
    target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with('#')
        || target.starts_with("mailto:")
}

/// Normalise a repo-relative path (resolve `.`/`..` lexically). Returns None if
/// it escapes above the repo root.
fn normalize_rel(base_dir: &Path, target: &str) -> Option<PathBuf> {
    let joined = base_dir.join(target);
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(s) => out.push(s),
            _ => return None,
        }
    }
    Some(out)
}

/// Rewrite a single doc's markdown: intra-set `.md` links → wikilinks; image
/// links → `_assets/...` copies (queued in `assets`). `doc_dir` is the doc's
/// repo-relative parent (e.g. `docs`).
fn rewrite(md: &str, doc_dir: &Path, assets: &mut Vec<AssetRef>) -> String {
    let re = link_re();
    let mut result = String::with_capacity(md.len());
    let mut last = 0;
    for cap in re.captures_iter(md) {
        let m = cap.get(0).unwrap();
        result.push_str(&md[last..m.start()]);
        last = m.end();

        let bang = &cap[1];
        let text = &cap[2];
        let target = &cap[3];

        if is_external(target) {
            result.push_str(m.as_str());
            continue;
        }

        if !bang.is_empty() {
            // Image: queue an asset copy and rewrite to the vault path.
            if let Some(repo_rel) = normalize_rel(doc_dir, target) {
                let vault_rel = Path::new("_assets").join(&repo_rel);
                if !assets.iter().any(|a| a.repo_rel == repo_rel) {
                    assets.push(AssetRef {
                        repo_rel: repo_rel.clone(),
                        vault_rel: vault_rel.clone(),
                    });
                }
                result.push_str(&format!("![{text}]({})", vault_rel.to_string_lossy()));
            } else {
                result.push_str(m.as_str());
            }
        } else if target.ends_with(".md") {
            // Intra-set doc link → Obsidian wikilink by note stem.
            let stem = Path::new(target)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| target.to_string());
            result.push_str(&format!("[[{stem}|{text}]]"));
        } else {
            result.push_str(m.as_str());
        }
    }
    result.push_str(&md[last..]);
    result
}

fn mirror_one(d: &SourceDoc, out: &mut RenderOutput) {
    let doc_dir = d.repo_rel.parent().unwrap_or(Path::new(""));
    let source = d.repo_rel.to_string_lossy().to_string();
    let body = rewrite(&d.content, doc_dir, &mut out.assets);
    out.notes.push(VaultNote {
        rel_path: d.repo_rel.clone(),
        markdown: format!("{}{}", front_matter(&source), body),
    });
}

pub fn docs_mirror(ctx: &RepoContext, out: &mut RenderOutput) {
    for d in &ctx.docs {
        mirror_one(d, out);
    }
    for d in &ctx.top_level {
        mirror_one(d, out);
    }
}
pub fn architecture(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn sessions(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn section_indexes(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn home_moc(_ctx: &RepoContext, _out: &mut RenderOutput) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{RenderOutput, RepoContext, SourceDoc};
    use std::path::PathBuf;

    fn doc(rel: &str, content: &str) -> SourceDoc {
        SourceDoc {
            repo_rel: PathBuf::from(rel),
            content: content.into(),
        }
    }

    #[test]
    fn mirrors_docs_and_rewrites_md_link_to_wikilink() {
        let ctx = RepoContext {
            repo_name: "entheai".into(),
            docs: vec![
                doc("docs/architecture.md", "See [the TUI](tui.md) for details."),
                doc("docs/tui.md", "# TUI\n"),
            ],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);

        let arch = out
            .notes
            .iter()
            .find(|n| n.rel_path == PathBuf::from("docs/architecture.md"))
            .expect("architecture note mirrored");
        assert!(arch.markdown.contains("generated_by: entheai"));
        assert!(
            arch.markdown.contains("[[tui|the TUI]]"),
            "intra-set .md link rewritten to a wikilink, got: {}",
            arch.markdown
        );
    }

    #[test]
    fn image_link_becomes_asset_copy_and_rewrite() {
        let ctx = RepoContext {
            repo_name: "entheai".into(),
            docs: vec![doc("docs/features.md", "![brain](images/brain.png)\n")],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);

        assert!(
            out.assets
                .iter()
                .any(|a| a.repo_rel == PathBuf::from("docs/images/brain.png")
                    && a.vault_rel == PathBuf::from("_assets/docs/images/brain.png")),
            "referenced image queued for copy, assets: {:?}",
            out.assets
        );
        let note = &out.notes[0];
        assert!(
            note.markdown
                .contains("![brain](_assets/docs/images/brain.png)"),
            "image link rewritten to the vault-relative asset path, got: {}",
            note.markdown
        );
    }

    #[test]
    fn top_level_readme_mirrored_when_present() {
        let ctx = RepoContext {
            repo_name: "entheai".into(),
            top_level: vec![doc("README.md", "# entheai\n")],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);
        assert!(out
            .notes
            .iter()
            .any(|n| n.rel_path == PathBuf::from("README.md")));
    }

    #[test]
    fn no_docs_no_notes() {
        let ctx = RepoContext {
            repo_name: "x".into(),
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);
        assert!(out.is_empty(), "per-source conditional: no docs → nothing");
    }
}
