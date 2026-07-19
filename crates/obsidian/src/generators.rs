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

/// If `target` is an intra-set doc link (`foo.md` or `foo.md#anchor`), split it
/// into (path_without_anchor, anchor). Otherwise None.
fn split_md_target(target: &str) -> Option<(&str, &str)> {
    let (path, anchor) = match target.split_once('#') {
        Some((p, a)) => (p, a),
        None => (target, ""),
    };
    path.ends_with(".md").then_some((path, anchor))
}

/// Rewrite links/images in a text span (no code protection — the caller is
/// responsible for only handing this unprotected text): intra-set `.md`
/// links → wikilinks (with optional `#anchor`, alias sanitized against `|`);
/// image links → `_assets/...` copies (queued in `assets`). `doc_dir` is the
/// doc's repo-relative parent (e.g. `docs`).
fn rewrite_text(md: &str, doc_dir: &Path, assets: &mut Vec<AssetRef>) -> String {
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
        } else if let Some((path, anchor)) = split_md_target(target) {
            // Intra-set doc link → Obsidian wikilink by note stem, optionally
            // pointing at a heading anchor; alias sanitized against `|`.
            let stem = Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());
            let alias = text.replace('|', "/");
            if anchor.is_empty() {
                result.push_str(&format!("[[{stem}|{alias}]]"));
            } else {
                result.push_str(&format!("[[{stem}#{anchor}|{alias}]]"));
            }
        } else {
            result.push_str(m.as_str());
        }
    }
    result.push_str(&md[last..]);
    result
}

/// Rewrite a single non-fenced line, protecting inline `code` spans (runs of N
/// backticks close on the next run of exactly N backticks) verbatim.
fn rewrite_line(line: &str, doc_dir: &Path, assets: &mut Vec<AssetRef>) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if bytes[i] == b'`' {
            let start = i;
            let mut n = 0;
            while i < line.len() && bytes[i] == b'`' {
                n += 1;
                i += 1;
            }
            // find a closing run of exactly n backticks
            let mut close = None;
            let mut j = i;
            while j < line.len() {
                if bytes[j] == b'`' {
                    let mut m = 0;
                    while j < line.len() && bytes[j] == b'`' {
                        m += 1;
                        j += 1;
                    }
                    if m == n {
                        close = Some(j);
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            match close {
                Some(end) => {
                    out.push_str(&line[start..end]);
                    i = end;
                }
                None => out.push_str(&line[start..i]),
            }
        } else {
            let tstart = i;
            while i < line.len() && bytes[i] != b'`' {
                i += 1;
            }
            out.push_str(&rewrite_text(&line[tstart..i], doc_dir, assets));
        }
    }
    out
}

/// Rewrite one doc: protect fenced code blocks (``` / ~~~) and inline code
/// spans (`...`) verbatim; rewrite links/images only in the remaining text.
fn rewrite(md: &str, doc_dir: &Path, assets: &mut Vec<AssetRef>) -> String {
    let mut result = String::with_capacity(md.len());
    let mut in_fence = false;
    for line in md.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            result.push_str(line);
            continue;
        }
        if in_fence {
            result.push_str(line);
            continue;
        }
        result.push_str(&rewrite_line(line, doc_dir, assets));
    }
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
pub fn architecture(ctx: &RepoContext, out: &mut RenderOutput) {
    if ctx.crates.is_empty() && ctx.repowise_index.is_none() {
        return; // degrade: nothing structural to describe
    }
    let mut md = front_matter("");
    md.push_str(&format!("# Architecture — {}\n\n", ctx.repo_name));
    if !ctx.crates.is_empty() {
        md.push_str("## Crates & binaries\n\n");
        for c in &ctx.crates {
            if c.role.is_empty() {
                md.push_str(&format!("- `{}`\n", c.name));
            } else {
                md.push_str(&format!("- `{}` — {}\n", c.name, c.role));
            }
        }
        md.push('\n');
    }
    if let Some(idx) = &ctx.repowise_index {
        md.push_str("## Codebase index (Repowise)\n\n");
        md.push_str(idx);
        md.push('\n');
    }
    out.notes.push(VaultNote {
        rel_path: PathBuf::from("Architecture.md"),
        markdown: md,
    });
}
pub fn sessions(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn section_indexes(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn home_moc(_ctx: &RepoContext, _out: &mut RenderOutput) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{CrateInfo, RenderOutput, RepoContext, SourceDoc};
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

    #[test]
    fn code_fence_content_is_not_rewritten() {
        let ctx = RepoContext {
            repo_name: "e".into(),
            docs: vec![doc(
                "docs/x.md",
                "```\n[the TUI](tui.md)\n![a](img.png)\n```\nreal [link](tui.md) here\n",
            )],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);
        let n = &out.notes[0];
        assert!(
            n.markdown.contains("[the TUI](tui.md)"),
            "fenced link left verbatim: {}",
            n.markdown
        );
        assert!(
            n.markdown.contains("[[tui|link]]"),
            "real link outside fence rewritten: {}",
            n.markdown
        );
        assert!(
            out.assets.is_empty(),
            "image inside a fence is NOT queued as an asset"
        );
    }

    #[test]
    fn inline_code_link_is_not_rewritten() {
        let ctx = RepoContext {
            repo_name: "e".into(),
            docs: vec![doc("docs/x.md", "use `[x](y.md)` but [real](y.md) yes\n")],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);
        let n = &out.notes[0];
        assert!(
            n.markdown.contains("`[x](y.md)`"),
            "inline-code link verbatim: {}",
            n.markdown
        );
        assert!(
            n.markdown.contains("[[y|real]]"),
            "real link rewritten: {}",
            n.markdown
        );
    }

    #[test]
    fn anchored_md_link_becomes_wikilink_with_heading() {
        let ctx = RepoContext {
            repo_name: "e".into(),
            docs: vec![doc("docs/x.md", "[sec](tui.md#usage)\n")],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);
        assert!(
            out.notes[0].markdown.contains("[[tui#usage|sec]]"),
            "got: {}",
            out.notes[0].markdown
        );
    }

    #[test]
    fn pipe_in_link_text_is_sanitized() {
        let ctx = RepoContext {
            repo_name: "e".into(),
            docs: vec![doc("docs/x.md", "[a|b](tui.md)\n")],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);
        assert!(
            out.notes[0].markdown.contains("[[tui|a/b]]"),
            "got: {}",
            out.notes[0].markdown
        );
    }

    #[test]
    fn architecture_lists_crates_and_folds_repowise() {
        let ctx = RepoContext {
            repo_name: "entheai".into(),
            crates: vec![
                CrateInfo {
                    name: "entheai-obsidian".into(),
                    role: "wiki-sync layer".into(),
                },
                CrateInfo {
                    name: "entheai-memory".into(),
                    role: "recall store".into(),
                },
            ],
            repowise_index: Some("### Hotspots\n- crates/core/src/lib.rs".into()),
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        architecture(&ctx, &mut out);
        let note = out
            .notes
            .iter()
            .find(|n| n.rel_path == PathBuf::from("Architecture.md"))
            .expect("architecture note present");
        assert!(note.markdown.contains("entheai-obsidian"));
        assert!(note.markdown.contains("wiki-sync layer"));
        assert!(
            note.markdown.contains("Hotspots"),
            "repowise index folded in"
        );
    }

    #[test]
    fn architecture_absent_when_no_crates_and_no_index() {
        let ctx = RepoContext {
            repo_name: "script".into(),
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        architecture(&ctx, &mut out);
        assert!(
            out.notes.is_empty(),
            "degrade: nothing to describe → no note"
        );
    }
}
