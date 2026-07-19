//! Pure render layer: `RepoContext` → `RenderOutput`. No I/O, no async.

use std::path::PathBuf;

/// One rendered markdown note. `rel_path` is relative to `<vault>/<subtree>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultNote {
    pub rel_path: PathBuf,
    pub markdown: String,
}

/// An asset (e.g. image) to copy from the repo into the vault subtree.
/// `repo_rel` is relative to the repo root; `vault_rel` to `<vault>/<subtree>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRef {
    pub repo_rel: PathBuf,
    pub vault_rel: PathBuf,
}

/// The complete pure render result for one repo scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderOutput {
    pub notes: Vec<VaultNote>,
    pub assets: Vec<AssetRef>,
}

impl RenderOutput {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.assets.is_empty()
    }
}

/// A source markdown file read from the repo.
#[derive(Debug, Clone)]
pub struct SourceDoc {
    /// Path relative to the repo root, e.g. `docs/architecture.md`.
    pub repo_rel: PathBuf,
    pub content: String,
}

/// One workspace crate/bin for the architecture note.
#[derive(Debug, Clone)]
pub struct CrateInfo {
    pub name: String,
    /// One-line role (from the crate's `//!` or `description`, else empty).
    pub role: String,
}

/// Toggles from config (`include_architecture`, `include_sessions`).
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub include_architecture: bool,
    pub include_sessions: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            include_architecture: true,
            include_sessions: true,
        }
    }
}

/// Everything the pure layer needs, gathered by `scan.rs`.
#[derive(Debug, Clone, Default)]
pub struct RepoContext {
    pub repo_name: String,
    pub docs: Vec<SourceDoc>,
    pub top_level: Vec<SourceDoc>,
    pub sessions: Vec<SourceDoc>,
    pub crates: Vec<CrateInfo>,
    pub repowise_index: Option<String>,
    pub specs: Vec<PathBuf>,
    pub plans: Vec<PathBuf>,
    pub research: Vec<PathBuf>,
    pub options: RenderOptionsHolder,
}

/// Wrapper so `RepoContext` can `derive(Default)` with our non-derivable options.
#[derive(Debug, Clone, Default)]
pub struct RenderOptionsHolder(pub RenderOptions);

/// Render the full vault content for a repo. Pure and deterministic.
/// Order of notes: docs mirror, architecture, sessions, indexes, then Home
/// (Home is appended last so it can reference the others' titles).
pub fn render_all(ctx: &RepoContext) -> RenderOutput {
    let mut out = RenderOutput::default();
    crate::generators::docs_mirror(ctx, &mut out);
    if ctx.options.0.include_architecture {
        crate::generators::architecture(ctx, &mut out);
    }
    if ctx.options.0.include_sessions {
        crate::generators::sessions(ctx, &mut out);
    }
    crate::generators::section_indexes(ctx, &mut out);
    crate::generators::home_moc(ctx, &mut out);
    out
}

/// Stamp YAML front-matter identifying an entheai-generated note.
/// `source` is the repo-relative origin (empty for synthesized notes).
pub fn front_matter(source: &str) -> String {
    let src_line = if source.is_empty() {
        String::new()
    } else {
        format!("source: {source}\n")
    };
    // `updated` is stamped by the writer at write time (§5); the pure layer
    // leaves a stable placeholder token the writer replaces, so identical
    // content hashes across renders (the timestamp must not perturb the hash).
    format!("---\ngenerated_by: entheai\n{src_line}updated: {{UPDATED}}\n---\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_context_renders_nothing() {
        // Activation floor: a bare repo (no docs, no sessions, no crates) yields
        // zero notes and zero assets, so the bin skips creating the subtree.
        let ctx = RepoContext {
            repo_name: "scratch".into(),
            ..Default::default()
        };
        let out = render_all(&ctx);
        assert!(
            out.is_empty(),
            "no sources → empty render → lazy (no subtree)"
        );
    }

    #[test]
    fn front_matter_stamps_generated_by_and_source() {
        let fm = front_matter("docs/architecture.md");
        assert!(fm.contains("generated_by: entheai"));
        assert!(fm.contains("source: docs/architecture.md"));
        assert!(fm.contains("updated: {UPDATED}"));
    }
}
