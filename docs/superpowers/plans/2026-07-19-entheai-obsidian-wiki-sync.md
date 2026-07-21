# entheai Obsidian Wiki-Sync Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When Obsidian is available locally, entheai keeps a per-project vault continuously synced as a generated, backlinked "project brain" — seeded on launch and updated live while entheai runs, one-way (repo + entheai → vault).

**Architecture:** A new `crates/obsidian` split into a **pure render layer** (`RepoContext → RenderOutput { notes, assets }`, no I/O, exhaustively unit-tested) and a thin **runtime layer** (`scan` FS→context, `resolve` vault detection, `VaultWriter` atomic managed-subtree writes with hash-skip + orphan GC + write-confinement, a `notify`-debounced `watcher`, a best-effort MCP `nudge`). `bin/entheai` spawns a session-scoped watcher via an RAII `ObsidianSession` guard that stops on drop. All errors are contained — the layer never blocks or crashes the agent.

**Tech Stack:** Rust, `walkdir` (repo scan), `regex` (link rewriting), `notify-debouncer-mini` (debounced FS events), `tempfile` (atomic writes), `tokio` + `tokio-tungstenite` (best-effort MCP nudge), inline FNV-1a (stable content hashing, no dep).

---

## Reference: spec

Full design: `docs/superpowers/specs/2026-07-19-entheai-obsidian-wiki-sync-design.md`. Read it once before starting.

## Repo hazard — read before every commit

This is the shared multi-session `main` checkout. Every task ends with a commit using **scoped `git add <exact paths>`** (never `git add -A`/`.`/`-u`) and **pushes immediately**:

```bash
git add <exact paths for this task>
git commit -m "<message>"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

- `crates/obsidian/**` and `Cargo.lock` are **new / collision-free**. `crates/config/src/lib.rs` and `bin/entheai/src/main.rs` are **HOT** (other sessions edit them) — re-read immediately before editing, and if the push is rejected, `git pull --rebase` then re-push; if the rebase conflicts, STOP and report.
- `Cargo.toml` (workspace root) is edited once (Task 0, add the member) — small, but re-read before editing.
- Never `git reset --hard`. Follow SemVer 0.x additive; **do not** bump `[workspace.package].version`.
- **Snyk:** CLAUDE.md asks for `snyk_code_scan` on new code. That MCP tool is unavailable in this environment — note "snyk_code_scan unavailable" in your report and proceed; do not block on it.

## Design decisions locked here (reconciling the spec)

1. **`render_all` only (no `render_changed`).** The watcher re-renders the whole `RepoContext` on each debounced batch and relies on the `VaultWriter`'s content-hash no-op skip + orphan GC for efficiency. This is simpler and more robust than targeted change→note mapping; a `render_changed` fast-path is a future optimization. Observable behavior (correct vault, minimal writes) is identical.
2. **Pure layer is byte-free for assets.** `render_all` returns `RenderOutput { notes: Vec<VaultNote>, assets: Vec<AssetRef> }`. An `AssetRef { repo_rel, vault_rel }` is a *reference* (image to copy repo→vault), not bytes — keeping the render layer pure. The `VaultWriter` (I/O) does the byte copy.
3. **`obsidian` does not depend on `entheai-config`.** It defines its own `ObsidianOptions`; the bin maps `entheai_config::ObsidianConfig → obsidian::ObsidianOptions` (mirrors how `bin` maps `MemoryConfig → MemoryRuntimeConfig`). Config → obsidian is one-way and cycle-free.
4. **Content hash is inline FNV-1a 64** (stable across runs/versions; the manifest persists between sessions). No `sha2`/`blake3` dependency.
5. **Memory-Highlights is OUT of scope** (v1.1 seam; `crates/memory` is @rahulmranga's). Leave a clearly-marked seam comment where it would plug in; do not build it.

## File structure

| File | Responsibility |
|------|----------------|
| `crates/obsidian/Cargo.toml` | crate manifest + deps |
| `crates/obsidian/src/lib.rs` | module decls, re-exports, `ObsidianOptions`, `ObsidianSession` RAII entry (`start`), the `run`/`apply` glue |
| `crates/obsidian/src/render.rs` | pure types (`VaultNote`, `AssetRef`, `RenderOutput`, `RepoContext`, `SourceDoc`, `CrateInfo`, `RenderOptions`), `render_all`, front-matter helper |
| `crates/obsidian/src/generators.rs` | pure per-note generators (docs mirror + link/asset rewrite, architecture, sessions, section indexes, Home MOC) |
| `crates/obsidian/src/scan.rs` | I/O: `scan(root, &ObsidianOptions) -> io::Result<RepoContext>` |
| `crates/obsidian/src/resolve.rs` | vault resolution (3 rules + `.obsidian/` guard) |
| `crates/obsidian/src/writer.rs` | `VaultWriter`: atomic writes, FNV manifest, no-op skip, asset copy, orphan GC, write-confinement |
| `crates/obsidian/src/watcher.rs` | `notify-debouncer-mini` wiring → batch ticks |
| `crates/obsidian/src/nudge.rs` | best-effort MCP WebSocket nudge |
| `Cargo.toml` (root) | add `crates/obsidian` to `members` |
| `crates/config/src/lib.rs` | add `ObsidianConfig` + `pub obsidian` field |
| `bin/entheai/src/main.rs` | map config→options; spawn/stop `ObsidianSession` per session |

---

## Task 0: crate scaffold + deps + `notify` de-risk gate

Prove the risky external dependency (`notify-debouncer-mini` delivers real FS events) links and works against this toolchain **before** building on it. Establish the crate.

**Files:**
- Create: `crates/obsidian/Cargo.toml`
- Create: `crates/obsidian/src/lib.rs`
- Modify: `Cargo.toml` (root — add member)
- Modify: `Cargo.lock` (regenerated)

- [ ] **Step 1: Create the crate manifest**

Create `crates/obsidian/Cargo.toml`:

```toml
[package]
name = "entheai-obsidian"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
anyhow = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
walkdir = { workspace = true }
tokio = { workspace = true, features = ["time", "sync"] }
regex = "1"
notify-debouncer-mini = "0.4"
tokio-tungstenite = "0.24"
futures-util = "0.3"
log = "0.4"
tempfile = "3"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create a minimal lib with the gate test**

Create `crates/obsidian/src/lib.rs`:

```rust
//! entheai Obsidian wiki-sync layer. See
//! docs/superpowers/specs/2026-07-19-entheai-obsidian-wiki-sync-design.md.

#[cfg(test)]
mod gate_tests {
    use notify_debouncer_mini::new_debouncer;
    use std::sync::mpsc;
    use std::time::Duration;

    /// De-risk gate: `notify-debouncer-mini` delivers a debounced FS event for a
    /// real file write. Ignored by default (touches the real filesystem + timing);
    /// run explicitly with `cargo test -p entheai-obsidian -- --ignored gate`.
    #[test]
    #[ignore]
    fn notify_debouncer_delivers_events_gate() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut debouncer =
            new_debouncer(Duration::from_millis(200), move |res| {
                let _ = tx.send(res);
            })
            .unwrap();
        debouncer
            .watcher()
            .watch(dir.path(), notify_debouncer_mini::notify::RecursiveMode::Recursive)
            .unwrap();

        std::fs::write(dir.path().join("hello.md"), b"hi").unwrap();

        let batch = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a debounced batch should arrive")
            .expect("batch is Ok");
        assert!(
            batch.iter().any(|e| e.path.ends_with("hello.md")),
            "the written file appears in the debounced batch"
        );
    }
}
```

- [ ] **Step 3: Register the crate in the workspace**

In the root `Cargo.toml`, add `"crates/obsidian"` to the `[workspace] members` array (place it next to `"crates/viz", "crates/launcher"`).

- [ ] **Step 4: Build + run the gate**

Run: `cargo build -p entheai-obsidian`
Expected: resolves the new deps and compiles.

Run: `cargo test -p entheai-obsidian -- --ignored notify_debouncer_delivers_events_gate`
Expected: PASS (`test result: ok. 1 passed`). If it flakes on timing, raise the `recv_timeout` — do **not** proceed until the gate passes; it validates the whole watcher approach.

- [ ] **Step 5: Record resolved versions + commit**

Note the resolved `notify-debouncer-mini` / `tokio-tungstenite` / `regex` versions from `Cargo.lock` in the commit message.

Run: `cargo fmt -p entheai-obsidian` and `cargo clippy -p entheai-obsidian -- -D warnings` → clean.

```bash
git add crates/obsidian/Cargo.toml crates/obsidian/src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(obsidian): scaffold crate + notify-debouncer gate test"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 1: pure render types + `render_all` orchestration + activation floor

Establish the byte-free data model and the empty-context behavior (activation floor: no sources → no notes).

**Files:**
- Create: `crates/obsidian/src/render.rs`
- Modify: `crates/obsidian/src/lib.rs`

- [ ] **Step 1: Write the failing test (in `render.rs`)**

Create `crates/obsidian/src/render.rs`:

```rust
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
        Self { include_architecture: true, include_sessions: true }
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
#[derive(Debug, Clone)]
pub struct RenderOptionsHolder(pub RenderOptions);
impl Default for RenderOptionsHolder {
    fn default() -> Self {
        RenderOptionsHolder(RenderOptions::default())
    }
}

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
        let ctx = RepoContext { repo_name: "scratch".into(), ..Default::default() };
        let out = render_all(&ctx);
        assert!(out.is_empty(), "no sources → empty render → lazy (no subtree)");
    }

    #[test]
    fn front_matter_stamps_generated_by_and_source() {
        let fm = front_matter("docs/architecture.md");
        assert!(fm.contains("generated_by: entheai"));
        assert!(fm.contains("source: docs/architecture.md"));
        assert!(fm.contains("updated: {UPDATED}"));
    }
}
```

Note the `{UPDATED}` token: the pure layer must be deterministic (identical input → identical output) so the writer's content-hash is stable. The writer replaces `{UPDATED}` with the real timestamp **after** hashing (Task 7).

- [ ] **Step 2: Wire the module (stub the generators so it compiles)**

In `crates/obsidian/src/lib.rs`, add above the tests:

```rust
pub mod generators;
pub mod render;

pub use render::{
    render_all, AssetRef, CrateInfo, RenderOptions, RenderOutput, RepoContext, SourceDoc, VaultNote,
};
```

Create `crates/obsidian/src/generators.rs` with empty stubs (filled in Tasks 2–4):

```rust
//! Pure per-note generators. Each is per-source conditional: a missing source
//! contributes nothing (never an error).

use crate::render::{RenderOutput, RepoContext};

pub fn docs_mirror(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn architecture(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn sessions(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn section_indexes(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn home_moc(_ctx: &RepoContext, _out: &mut RenderOutput) {}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p entheai-obsidian render::`
Expected: PASS (2 tests) — empty context is empty, front-matter stamps correctly.

- [ ] **Step 4: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/render.rs crates/obsidian/src/generators.rs crates/obsidian/src/lib.rs
git commit -m "feat(obsidian): pure render types + render_all orchestration + activation floor"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 2: docs-mirror generator (wikilink + image-asset rewriting)

Mirror `docs/**.md` + top-level docs into notes, rewriting intra-set `.md` links to `[[wikilinks]]` and image links to copied `_assets/` references.

**Files:**
- Modify: `crates/obsidian/src/generators.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/obsidian/src/generators.rs` a `#[cfg(test)] mod tests` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{RenderOutput, RepoContext, SourceDoc};
    use std::path::PathBuf;

    fn doc(rel: &str, content: &str) -> SourceDoc {
        SourceDoc { repo_rel: PathBuf::from(rel), content: content.into() }
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
            docs: vec![doc(
                "docs/features.md",
                "![brain](images/brain.png)\n",
            )],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);

        assert!(
            out.assets.iter().any(|a| a.repo_rel == PathBuf::from("docs/images/brain.png")
                && a.vault_rel == PathBuf::from("_assets/docs/images/brain.png")),
            "referenced image queued for copy, assets: {:?}",
            out.assets
        );
        let note = &out.notes[0];
        assert!(
            note.markdown.contains("![brain](_assets/docs/images/brain.png)"),
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
        assert!(out.notes.iter().any(|n| n.rel_path == PathBuf::from("README.md")));
    }

    #[test]
    fn no_docs_no_notes() {
        let ctx = RepoContext { repo_name: "x".into(), ..Default::default() };
        let mut out = RenderOutput::default();
        docs_mirror(&ctx, &mut out);
        assert!(out.is_empty(), "per-source conditional: no docs → nothing");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-obsidian generators::tests`
Expected: FAIL — `docs_mirror` is a no-op stub.

- [ ] **Step 3: Implement `docs_mirror` + the link/asset rewriter**

Replace the `docs_mirror` stub and add helpers in `crates/obsidian/src/generators.rs`:

```rust
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
                    assets.push(AssetRef { repo_rel: repo_rel.clone(), vault_rel: vault_rel.clone() });
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
```

Remove the old `use crate::render::{RenderOutput, RepoContext};` line at the top of the file if it now duplicates the fuller `use` above (keep a single `use` for the render types).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-obsidian generators::tests`
Expected: PASS (4 tests).

- [ ] **Step 5: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/generators.rs
git commit -m "feat(obsidian): docs-mirror generator with wikilink + image-asset rewriting"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 3: architecture generator (crate layout + Repowise fold, degrade off-cargo)

Generate `Architecture.md` from the crate/bin layout, folding in the CLAUDE.md/Repowise index when present. Degrade to a plain listing when there are no crates.

**Files:**
- Modify: `crates/obsidian/src/generators.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod:

```rust
    use crate::render::CrateInfo;

    #[test]
    fn architecture_lists_crates_and_folds_repowise() {
        let ctx = RepoContext {
            repo_name: "entheai".into(),
            crates: vec![
                CrateInfo { name: "entheai-obsidian".into(), role: "wiki-sync layer".into() },
                CrateInfo { name: "entheai-memory".into(), role: "recall store".into() },
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
        assert!(note.markdown.contains("Hotspots"), "repowise index folded in");
    }

    #[test]
    fn architecture_absent_when_no_crates_and_no_index() {
        let ctx = RepoContext { repo_name: "script".into(), ..Default::default() };
        let mut out = RenderOutput::default();
        architecture(&ctx, &mut out);
        assert!(out.notes.is_empty(), "degrade: nothing to describe → no note");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-obsidian architecture`
Expected: FAIL — `architecture` is a stub.

- [ ] **Step 3: Implement `architecture`**

Replace the `architecture` stub:

```rust
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
    out.notes.push(VaultNote { rel_path: PathBuf::from("Architecture.md"), markdown: md });
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-obsidian architecture`
Expected: PASS (2 tests).

- [ ] **Step 5: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/generators.rs
git commit -m "feat(obsidian): architecture generator with Repowise fold + off-cargo degrade"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 4: sessions + section indexes + Home MOC (pure layer complete)

Session notes from `.remember/`, `Specs-and-Plans.md`/`Research.md` indexes, and the `Home.md` Map-of-Content that backlinks every section.

**Files:**
- Modify: `crates/obsidian/src/generators.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod:

```rust
    #[test]
    fn sessions_become_dated_notes_under_sessions_dir() {
        let ctx = RepoContext {
            repo_name: "entheai".into(),
            sessions: vec![doc(".remember/today-2026-07-19.md", "did things")],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        sessions(&ctx, &mut out);
        assert!(out
            .notes
            .iter()
            .any(|n| n.rel_path == PathBuf::from("Sessions/today-2026-07-19.md")
                && n.markdown.contains("did things")));
    }

    #[test]
    fn section_indexes_link_specs_and_research() {
        let ctx = RepoContext {
            repo_name: "entheai".into(),
            specs: vec![PathBuf::from("docs/superpowers/specs/2026-07-19-x-design.md")],
            research: vec![PathBuf::from("docs/research/deepresearch.md")],
            ..Default::default()
        };
        let mut out = RenderOutput::default();
        section_indexes(&ctx, &mut out);
        let specs = out
            .notes
            .iter()
            .find(|n| n.rel_path == PathBuf::from("Specs-and-Plans.md"))
            .unwrap();
        assert!(specs.markdown.contains("[[2026-07-19-x-design]]"));
        assert!(out.notes.iter().any(|n| n.rel_path == PathBuf::from("Research.md")));
    }

    #[test]
    fn home_moc_backlinks_present_sections() {
        // A context with docs + sessions should yield a Home linking both.
        let ctx = RepoContext {
            repo_name: "entheai".into(),
            docs: vec![doc("docs/tui.md", "# TUI")],
            sessions: vec![doc(".remember/now.md", "buffer")],
            ..Default::default()
        };
        let out = render_all(&ctx);
        let home = out
            .notes
            .iter()
            .find(|n| n.rel_path == PathBuf::from("Home.md"))
            .expect("Home present");
        assert!(home.markdown.contains(&format!("entheai — {}", ctx.repo_name)));
        assert!(home.markdown.contains("[[tui]]"), "links a mirrored doc");
        assert!(home.markdown.contains("Sessions"), "references the sessions section");
    }
```

Add `use crate::render::render_all;` to the `tests` mod imports.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-obsidian generators::tests`
Expected: FAIL — `sessions`, `section_indexes`, `home_moc` are stubs.

- [ ] **Step 3: Implement the three generators**

Replace the `sessions`, `section_indexes`, and `home_moc` stubs:

```rust
pub fn sessions(ctx: &RepoContext, out: &mut RenderOutput) {
    for s in &ctx.sessions {
        let name = s
            .repo_rel
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "session.md".into());
        let source = s.repo_rel.to_string_lossy().to_string();
        out.notes.push(VaultNote {
            rel_path: PathBuf::from("Sessions").join(name),
            markdown: format!("{}{}", front_matter(&source), s.content),
        });
    }
}

/// Render one index note listing `paths` as wikilinks by stem. Skips when empty.
fn index_note(title: &str, rel_path: &str, paths: &[PathBuf], out: &mut RenderOutput) {
    if paths.is_empty() {
        return;
    }
    let mut md = front_matter("");
    md.push_str(&format!("# {title}\n\n"));
    for p in paths {
        if let Some(stem) = p.file_stem() {
            md.push_str(&format!("- [[{}]]\n", stem.to_string_lossy()));
        }
    }
    out.notes.push(VaultNote { rel_path: PathBuf::from(rel_path), markdown: md });
}

pub fn section_indexes(ctx: &RepoContext, out: &mut RenderOutput) {
    // Specs + plans share one index.
    let mut specs_plans = ctx.specs.clone();
    specs_plans.extend(ctx.plans.iter().cloned());
    index_note("Specs & Plans", "Specs-and-Plans.md", &specs_plans, out);
    index_note("Research", "Research.md", &ctx.research, out);
}

pub fn home_moc(ctx: &RepoContext, out: &mut RenderOutput) {
    // Home is generated last and inspects what other generators already produced.
    if out.notes.is_empty() {
        return; // lazy: nothing to link
    }
    let mut md = front_matter("");
    md.push_str(&format!("# entheai — {}\n\n", ctx.repo_name));
    md.push_str("Generated project wiki. Sections:\n\n");

    let has = |p: &str| out.notes.iter().any(|n| n.rel_path == PathBuf::from(p));

    if !ctx.docs.is_empty() || !ctx.top_level.is_empty() {
        md.push_str("## Docs\n\n");
        for d in ctx.docs.iter().chain(ctx.top_level.iter()) {
            if let Some(stem) = d.repo_rel.file_stem() {
                md.push_str(&format!("- [[{}]]\n", stem.to_string_lossy()));
            }
        }
        md.push('\n');
    }
    if has("Architecture.md") {
        md.push_str("## [[Architecture]]\n\n");
    }
    if out.notes.iter().any(|n| n.rel_path.starts_with("Sessions")) {
        md.push_str("## Sessions\n\n");
        for n in out.notes.iter().filter(|n| n.rel_path.starts_with("Sessions")) {
            if let Some(stem) = n.rel_path.file_stem() {
                md.push_str(&format!("- [[{}]]\n", stem.to_string_lossy()));
            }
        }
        md.push('\n');
    }
    if has("Specs-and-Plans.md") {
        md.push_str("## [[Specs-and-Plans|Specs & Plans]]\n\n");
    }
    if has("Research.md") {
        md.push_str("## [[Research]]\n\n");
    }
    // SEAM(v1.1, @rahulmranga): when the SQLite memory layer is wired, add a
    // "## [[Memory-Highlights]]" section here + a Memory-Highlights.md generator
    // that reads top learnings read-only from crates/memory. Out of scope now.

    out.notes.push(VaultNote { rel_path: PathBuf::from("Home.md"), markdown: md });
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-obsidian`
Expected: PASS — all render/generator tests (the pure layer is now complete).

- [ ] **Step 5: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/generators.rs
git commit -m "feat(obsidian): sessions + section indexes + Home MOC — pure render layer complete"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 5: `scan.rs` — filesystem → `RepoContext`

The I/O that gathers a `RepoContext` from a repo root (docs, top-level, `.remember/`, crate layout, Repowise index, spec/plan/research paths).

**Files:**
- Create: `crates/obsidian/src/scan.rs`
- Modify: `crates/obsidian/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/obsidian/src/scan.rs`:

```rust
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
    let crates = if options.include_architecture { read_crates(root) } else { Vec::new() };
    let repowise_index = std::fs::read_to_string(root.join("CLAUDE.md")).ok().or_else(|| {
        std::fs::read_to_string(root.join(".claude").join("CLAUDE.md")).ok()
    });
    let specs = list_markdown(&root.join("docs/superpowers/specs"));
    let plans = list_markdown(&root.join("docs/superpowers/plans"));
    let research = list_markdown(&root.join("docs/research"));

    Ok(RepoContext {
        repo_name,
        docs,
        top_level,
        sessions,
        crates,
        repowise_index,
        specs,
        plans,
        research,
        options: RenderOptionsHolder(options),
    })
}

fn read_doc(root: &Path, rel: &Path) -> Option<SourceDoc> {
    let content = std::fs::read_to_string(root.join(rel)).ok()?;
    Some(SourceDoc { repo_rel: rel.to_path_buf(), content })
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
                out.push(SourceDoc { repo_rel: rel.to_path_buf(), content });
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
        assert!(ctx.crates.iter().any(|c| c.name == "foo" && c.role == "the foo crate"));
    }

    #[test]
    fn bare_dir_scans_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = scan(dir.path(), RenderOptions::default()).unwrap();
        assert!(ctx.docs.is_empty() && ctx.sessions.is_empty() && ctx.crates.is_empty());
    }
}
```

- [ ] **Step 2: Wire the module + run**

Add to `crates/obsidian/src/lib.rs`:

```rust
pub mod scan;
```

Run: `cargo test -p entheai-obsidian scan::`
Expected: PASS (2 tests).

- [ ] **Step 3: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/scan.rs crates/obsidian/src/lib.rs
git commit -m "feat(obsidian): scan.rs — filesystem to RepoContext"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 6: `resolve.rs` — vault resolution + `.obsidian/` guard

Resolve the vault for a repo (config path → auto-detect iCloud → none), requiring a real vault (`.obsidian/` present).

**Files:**
- Create: `crates/obsidian/src/resolve.rs`
- Modify: `crates/obsidian/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/obsidian/src/resolve.rs`:

```rust
//! Vault resolution (spec §4). Detection only — no writes.

use std::path::{Path, PathBuf};

/// Resolve the vault directory for `repo_root`, honoring an explicit override.
/// Rules (first hit wins): (1) `vault_path_override` if non-empty and a valid
/// vault; (2) `~/Library/Mobile Documents/iCloud~md~obsidian/<repo-name>` if a
/// valid vault; (3) None. A "valid vault" is a directory containing `.obsidian/`.
/// `home` is injected for testability (the bin passes the real `$HOME`).
pub fn resolve_vault(repo_root: &Path, vault_path_override: &str, home: &Path) -> Option<PathBuf> {
    if !vault_path_override.is_empty() {
        let p = expand_home(vault_path_override, home);
        return is_vault(&p).then_some(p);
    }
    let name = repo_root.file_name()?;
    let candidate = home
        .join("Library/Mobile Documents/iCloud~md~obsidian")
        .join(name);
    is_vault(&candidate).then_some(candidate)
}

/// A directory is a vault iff it contains a `.obsidian/` subdirectory.
fn is_vault(dir: &Path) -> bool {
    dir.join(".obsidian").is_dir()
}

fn expand_home(path: &str, home: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vault(at: &Path) {
        std::fs::create_dir_all(at.join(".obsidian")).unwrap();
    }

    #[test]
    fn explicit_override_wins_when_valid() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("myvault");
        make_vault(&vault);
        let got = resolve_vault(
            Path::new("/whatever/repo"),
            vault.to_str().unwrap(),
            dir.path(),
        );
        assert_eq!(got.as_deref(), Some(vault.as_path()));
    }

    #[test]
    fn autodetects_icloud_vault_by_repo_name() {
        let home = tempfile::tempdir().unwrap();
        let vault = home
            .path()
            .join("Library/Mobile Documents/iCloud~md~obsidian/entheai");
        make_vault(&vault);
        let got = resolve_vault(Path::new("/x/entheai"), "", home.path());
        assert_eq!(got.as_deref(), Some(vault.as_path()));
    }

    #[test]
    fn same_named_dir_without_dot_obsidian_is_not_a_vault() {
        let home = tempfile::tempdir().unwrap();
        // Directory exists but has no `.obsidian/` → not a vault.
        std::fs::create_dir_all(
            home.path().join("Library/Mobile Documents/iCloud~md~obsidian/entheai"),
        )
        .unwrap();
        assert!(resolve_vault(Path::new("/x/entheai"), "", home.path()).is_none());
    }

    #[test]
    fn none_when_nothing_resolves() {
        let home = tempfile::tempdir().unwrap();
        assert!(resolve_vault(Path::new("/x/entheai"), "", home.path()).is_none());
    }
}
```

- [ ] **Step 2: Wire the module + run**

Add to `crates/obsidian/src/lib.rs`:

```rust
pub mod resolve;
```

Run: `cargo test -p entheai-obsidian resolve::`
Expected: PASS (4 tests).

- [ ] **Step 3: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/resolve.rs crates/obsidian/src/lib.rs
git commit -m "feat(obsidian): resolve.rs — vault resolution with .obsidian guard"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 7: `writer.rs` — atomic managed-subtree writes, hash-skip, asset copy, orphan GC, confinement

The only code that mutates the vault. Enforces the hard invariant: nothing is ever written or deleted outside the managed subtree.

**Files:**
- Create: `crates/obsidian/src/writer.rs`
- Modify: `crates/obsidian/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/obsidian/src/writer.rs`:

```rust
//! Managed-subtree vault writer (spec §7). FS-direct, atomic, hash-skipped,
//! orphan-GC'd, and confined to `<subtree>/`.

use crate::render::RenderOutput;
use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

const MANIFEST: &str = ".entheai-sync-manifest.json";

/// Writes generated notes/assets into `<vault>/<subtree>/`, tracking a
/// per-path content hash so unchanged notes are not rewritten and vanished
/// sources are garbage-collected.
pub struct VaultWriter {
    subtree: PathBuf,
    /// rel_path (as string) → content hash of the last write.
    manifest: BTreeMap<String, u64>,
    /// rel_paths written during the most recent `apply` (for the nudge).
    last_changed: Vec<PathBuf>,
}

impl VaultWriter {
    /// Open (creating lazily on first write) a writer rooted at `<vault>/<subtree>/`.
    pub fn new(subtree: PathBuf) -> Self {
        let manifest = load_manifest(&subtree);
        Self { subtree, manifest, last_changed: Vec::new() }
    }

    pub fn last_changed(&self) -> &[PathBuf] {
        &self.last_changed
    }

    /// Write the render output: create/update changed notes, copy referenced
    /// assets from `repo_root`, delete orphaned generated files, persist the
    /// manifest. A no-op render (empty output) creates nothing (lazy subtree).
    pub fn apply(&mut self, out: &RenderOutput, repo_root: &Path) -> io::Result<()> {
        self.last_changed.clear();
        if out.is_empty() && self.manifest.is_empty() {
            return Ok(()); // lazy: never materialize an empty subtree
        }

        let mut desired: BTreeMap<String, u64> = BTreeMap::new();

        // Notes.
        for note in &out.notes {
            let key = rel_key(&note.rel_path);
            let hash = fnv1a(note.markdown.as_bytes());
            desired.insert(key.clone(), hash);
            if self.manifest.get(&key) != Some(&hash) {
                let body = stamp_updated(&note.markdown);
                self.write_confined(&note.rel_path, body.as_bytes())?;
                self.last_changed.push(note.rel_path.clone());
            }
        }

        // Assets (copy bytes from the repo; hash the source path + len cheaply).
        for asset in &out.assets {
            let key = rel_key(&asset.vault_rel);
            let src = repo_root.join(&asset.repo_rel);
            let bytes = match std::fs::read(&src) {
                Ok(b) => b,
                Err(_) => continue, // missing source asset: skip, not fatal
            };
            let hash = fnv1a(&bytes);
            desired.insert(key.clone(), hash);
            if self.manifest.get(&key) != Some(&hash) {
                self.write_confined(&asset.vault_rel, &bytes)?;
                self.last_changed.push(asset.vault_rel.clone());
            }
        }

        // Orphan GC: anything in the old manifest but not desired.
        let orphans: Vec<String> =
            self.manifest.keys().filter(|k| !desired.contains_key(*k)).cloned().collect();
        for key in orphans {
            self.delete_confined(Path::new(&key))?;
        }

        self.manifest = desired;
        self.persist_manifest()?;
        Ok(())
    }

    /// Join `rel` under the subtree, refusing any path that escapes it.
    fn safe_target(&self, rel: &Path) -> io::Result<PathBuf> {
        for c in rel.components() {
            match c {
                Component::Normal(_) | Component::CurDir => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("refusing non-confined vault path: {}", rel.display()),
                    ))
                }
            }
        }
        Ok(self.subtree.join(rel))
    }

    fn write_confined(&self, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        let target = self.safe_target(rel)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Atomic: write a temp file in the same dir, then persist (rename).
        let dir = target.parent().unwrap_or(&self.subtree);
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        io::Write::write_all(&mut tmp, bytes)?;
        tmp.persist(&target).map_err(|e| e.error)?;
        Ok(())
    }

    fn delete_confined(&self, rel: &Path) -> io::Result<()> {
        let target = self.safe_target(rel)?;
        match std::fs::remove_file(&target) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn persist_manifest(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.subtree)?;
        let json = serde_json::to_string_pretty(&self.manifest)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        std::fs::write(self.subtree.join(MANIFEST), json)
    }
}

fn load_manifest(subtree: &Path) -> BTreeMap<String, u64> {
    std::fs::read_to_string(subtree.join(MANIFEST))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn rel_key(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Replace the `{UPDATED}` front-matter token with a fixed synthetic timestamp.
/// NOTE: real wall-clock is injected by the runtime; tests use this stable form.
fn stamp_updated(md: &str) -> String {
    md.replace("{UPDATED}", &now_iso8601())
}

/// Current time as ISO-8601. Isolated here so tests can rely on a fixed value
/// via the `test` cfg (avoids nondeterministic hashing — the token is replaced
/// AFTER hashing, so the timestamp never affects change detection).
fn now_iso8601() -> String {
    #[cfg(test)]
    {
        "1970-01-01T00:00:00Z".to_string()
    }
    #[cfg(not(test))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("epoch:{secs}")
    }
}

/// Stable FNV-1a 64-bit hash (persisted across sessions — must not depend on
/// Rust's per-run `DefaultHasher`).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{AssetRef, VaultNote};

    fn note(rel: &str, md: &str) -> VaultNote {
        VaultNote { rel_path: PathBuf::from(rel), markdown: md.into() }
    }

    #[test]
    fn writes_note_then_second_identical_apply_is_noop() {
        let vault = tempfile::tempdir().unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());

        let out = RenderOutput { notes: vec![note("Home.md", "hi {UPDATED}")], assets: vec![] };
        w.apply(&out, vault.path()).unwrap();
        assert!(subtree.join("Home.md").is_file());
        assert_eq!(w.last_changed().len(), 1);

        // Re-open + re-apply identical content → no rewrite (hash-skip).
        let mut w2 = VaultWriter::new(subtree.clone());
        w2.apply(&out, vault.path()).unwrap();
        assert!(w2.last_changed().is_empty(), "unchanged note not rewritten");
    }

    #[test]
    fn orphan_note_is_deleted_when_source_vanishes() {
        let vault = tempfile::tempdir().unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());
        w.apply(&RenderOutput { notes: vec![note("A.md", "a")], assets: vec![] }, vault.path())
            .unwrap();
        assert!(subtree.join("A.md").is_file());
        // Next render no longer includes A.md → GC removes it.
        w.apply(&RenderOutput { notes: vec![note("B.md", "b")], assets: vec![] }, vault.path())
            .unwrap();
        assert!(!subtree.join("A.md").exists(), "orphan removed");
        assert!(subtree.join("B.md").is_file());
    }

    #[test]
    fn refuses_to_write_outside_subtree() {
        let vault = tempfile::tempdir().unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());
        let out = RenderOutput { notes: vec![note("../escape.md", "nope")], assets: vec![] };
        let err = w.apply(&out, vault.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!vault.path().join("escape.md").exists(), "nothing written outside subtree");
    }

    #[test]
    fn copies_referenced_asset_into_subtree() {
        let vault = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("docs/images")).unwrap();
        std::fs::write(repo.path().join("docs/images/brain.png"), b"PNGDATA").unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());
        let out = RenderOutput {
            notes: vec![],
            assets: vec![AssetRef {
                repo_rel: PathBuf::from("docs/images/brain.png"),
                vault_rel: PathBuf::from("_assets/docs/images/brain.png"),
            }],
        };
        w.apply(&out, repo.path()).unwrap();
        assert_eq!(
            std::fs::read(subtree.join("_assets/docs/images/brain.png")).unwrap(),
            b"PNGDATA"
        );
    }
}
```

- [ ] **Step 2: Wire the module + run**

Add to `crates/obsidian/src/lib.rs`:

```rust
pub mod writer;
```

Run: `cargo test -p entheai-obsidian writer::`
Expected: PASS (4 tests).

- [ ] **Step 3: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/writer.rs crates/obsidian/src/lib.rs
git commit -m "feat(obsidian): VaultWriter — atomic writes, hash-skip, asset copy, orphan GC, confinement"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 8: `watcher.rs` — debounced FS events → apply pipeline

Wire `notify-debouncer-mini` to a channel of batch "ticks"; the runtime drains them and re-applies. The tested unit is the `apply` pipeline (scan → render → write); the notify wiring is thin and `#[ignore]`-integration-tested.

**Files:**
- Create: `crates/obsidian/src/watcher.rs`
- Modify: `crates/obsidian/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/obsidian/src/watcher.rs`:

```rust
//! notify-debouncer wiring. The heavy lifting (scan→render→write) is
//! `crate::apply`; this module only turns FS activity into debounced ticks.

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;

/// Holds the debouncer alive; dropping it stops watching.
pub struct Watcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

/// Watch the configured paths under `root`. Every debounced batch of FS events
/// sends one `()` tick on `tick_tx`. `watch` is the list of repo-relative paths
/// (dirs or files) to observe; missing ones are skipped.
pub fn spawn(
    root: &Path,
    watch: &[String],
    debounce: Duration,
    tick_tx: mpsc::UnboundedSender<()>,
) -> anyhow::Result<Watcher> {
    let mut debouncer = new_debouncer(debounce, move |res: DebounceEventResult| {
        if res.is_ok() {
            let _ = tick_tx.send(());
        }
    })?;
    for rel in watch {
        let p = root.join(rel);
        if p.exists() {
            // A missing path can't be watched; that's fine (per-source conditional).
            let _ = debouncer.watcher().watch(&p, RecursiveMode::Recursive);
        }
    }
    Ok(Watcher { _debouncer: debouncer })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn real_watch_ticks_on_write() {
        // Integration-style but fast: uses a short debounce. Marked ignore-free
        // because it is bounded (<2s) and deterministic on macOS FSEvents.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _w = spawn(
            dir.path(),
            &["docs".to_string()],
            Duration::from_millis(150),
            tx,
        )
        .unwrap();

        std::fs::write(dir.path().join("docs/x.md"), b"hi").unwrap();

        let ticked = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
        assert!(ticked.is_ok() && ticked.unwrap().is_some(), "a write produced a tick");
    }
}
```

- [ ] **Step 2: Wire the module + run**

Add to `crates/obsidian/src/lib.rs`:

```rust
pub mod watcher;
```

Run: `cargo test -p entheai-obsidian watcher::`
Expected: PASS. If the FS-event test is flaky in CI/sandbox, add `#[ignore]` to `real_watch_ticks_on_write` and document running it manually — but on a real macOS dev box it should pass reliably.

- [ ] **Step 3: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/watcher.rs crates/obsidian/src/lib.rs
git commit -m "feat(obsidian): notify-debounced watcher → tick channel"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 9: `nudge.rs` — best-effort MCP WebSocket nudge

Fire-and-forget refresh to the `obsidian-claude-code-mcp` plugin. The **only** hard contract (and the tested one): an unreachable socket returns `Ok(())` and never errors or blocks.

**Files:**
- Create: `crates/obsidian/src/nudge.rs`
- Modify: `crates/obsidian/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/obsidian/src/nudge.rs`:

```rust
//! Best-effort nudge to the obsidian-claude-code-mcp WebSocket (spec §7).
//! Every failure (socket down, Obsidian closed, timeout) is swallowed.

use std::path::Path;
use std::time::Duration;

/// Nudge Obsidian to refresh `changed` notes under the vault, if the plugin is
/// listening on `127.0.0.1:<port>`. Never errors — returns `Ok(())` regardless.
/// The `open`-file method name is the plugin's convention and may be adjusted
/// once verified against the live plugin; correctness here is the swallow, not
/// the plugin accepting the message.
pub async fn best_effort(port: u16, vault_subtree: &Path, changed: &[std::path::PathBuf]) {
    if changed.is_empty() {
        return;
    }
    // Bounded so a hung socket never stalls the session.
    let _ = tokio::time::timeout(Duration::from_millis(400), try_nudge(port, vault_subtree, changed))
        .await;
}

async fn try_nudge(port: u16, vault_subtree: &Path, changed: &[std::path::PathBuf]) {
    use futures_util::SinkExt;
    let url = format!("ws://127.0.0.1:{port}");
    let Ok(Ok((mut ws, _))) = tokio::time::timeout(
        Duration::from_millis(200),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    else {
        return; // socket down / not a websocket → swallow
    };
    for rel in changed {
        let abs = vault_subtree.join(rel);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "obsidian/openFile",
            "params": { "path": abs.to_string_lossy() }
        })
        .to_string();
        let _ = ws.send(tokio_tungstenite::tungstenite::Message::Text(msg)).await;
    }
    let _ = ws.close(None).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn socket_down_is_swallowed() {
        // Nothing listening on this port → must return without panicking/erroring
        // and within the timeout budget.
        best_effort(0, Path::new("/tmp/nonexistent-vault"), &[PathBuf::from("Home.md")]).await;
        // Reaching here means no panic and no hang: the contract holds.
    }

    #[tokio::test]
    async fn empty_changed_is_a_fast_noop() {
        best_effort(22360, Path::new("/tmp/whatever"), &[]).await;
    }
}
```

- [ ] **Step 2: Wire the module + run**

Add to `crates/obsidian/src/lib.rs`:

```rust
pub mod nudge;
```

Run: `cargo test -p entheai-obsidian nudge::`
Expected: PASS (2 tests) within a couple of seconds.

- [ ] **Step 3: Gate + commit**

Run: `cargo clippy -p entheai-obsidian -- -D warnings` → clean. `cargo fmt -p entheai-obsidian`.

```bash
git add crates/obsidian/src/nudge.rs crates/obsidian/src/lib.rs
git commit -m "feat(obsidian): best-effort MCP WebSocket nudge (swallow on failure)"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 10: config — `[obsidian]` block

Add `ObsidianConfig` to `crates/config`, mirroring the `MemoryConfig` idiom (`#[serde(default = "fn")]` + `impl Default`).

**Files:**
- Modify: `crates/config/src/lib.rs`

- [ ] **Step 1: Write the failing defaults test**

`crates/config/src/lib.rs` is HOT — re-read it now, then add a test near the existing `memory_config_defaults` test:

```rust
#[test]
fn obsidian_config_defaults() {
    let cfg = Config::from_toml_str("").unwrap();
    assert!(cfg.obsidian.enabled, "obsidian on by default (no-op unless a vault resolves)");
    assert_eq!(cfg.obsidian.vault_path, "");
    assert_eq!(cfg.obsidian.subtree, "entheai-sync");
    assert_eq!(cfg.obsidian.debounce_ms, 500);
    assert!(cfg.obsidian.mcp_nudge);
    assert_eq!(cfg.obsidian.mcp_port, 22360);
    assert!(cfg.obsidian.include_architecture);
    assert!(cfg.obsidian.include_sessions);
    assert_eq!(
        cfg.obsidian.watch,
        vec!["docs", ".remember", "README.md", "AGENTS.md", "CHANGELOG.md", "VERSIONING.md"]
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-config obsidian_config_defaults`
Expected: FAIL — no `obsidian` field / `ObsidianConfig`.

- [ ] **Step 3: Implement the config block**

Add the field to `struct Config` (after `pub telemetry: TelemetryConfig,` at line ~43):

```rust
    #[serde(default)]
    pub obsidian: ObsidianConfig,
```

Add the struct + defaults + `impl Default` (place it near `MemoryConfig`, after that struct's `impl`/default fns):

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ObsidianConfig {
    #[serde(default = "default_obsidian_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub vault_path: String,
    #[serde(default = "default_obsidian_subtree")]
    pub subtree: String,
    #[serde(default = "default_obsidian_watch")]
    pub watch: Vec<String>,
    #[serde(default = "default_obsidian_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default = "default_true_obsidian")]
    pub mcp_nudge: bool,
    #[serde(default = "default_obsidian_mcp_port")]
    pub mcp_port: u16,
    #[serde(default = "default_true_obsidian")]
    pub include_architecture: bool,
    #[serde(default = "default_true_obsidian")]
    pub include_sessions: bool,
}

fn default_obsidian_enabled() -> bool {
    true
}
fn default_true_obsidian() -> bool {
    true
}
fn default_obsidian_subtree() -> String {
    "entheai-sync".into()
}
fn default_obsidian_debounce_ms() -> u64 {
    500
}
fn default_obsidian_mcp_port() -> u16 {
    22360
}
fn default_obsidian_watch() -> Vec<String> {
    ["docs", ".remember", "README.md", "AGENTS.md", "CHANGELOG.md", "VERSIONING.md"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

impl Default for ObsidianConfig {
    fn default() -> Self {
        Self {
            enabled: default_obsidian_enabled(),
            vault_path: String::new(),
            subtree: default_obsidian_subtree(),
            watch: default_obsidian_watch(),
            debounce_ms: default_obsidian_debounce_ms(),
            mcp_nudge: true,
            mcp_port: default_obsidian_mcp_port(),
            include_architecture: true,
            include_sessions: true,
        }
    }
}
```

(If a `default_true` fn already exists in this file, reuse it instead of `default_true_obsidian` — check first to avoid a duplicate; the `RouterConfig`/`CompanionConfig` blocks used `default_true`. If present, replace `default_true_obsidian` with `default_true` and drop the new fn.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-config obsidian_config_defaults`
Expected: PASS.

- [ ] **Step 5: Full crate gate + commit**

Run: `cargo test -p entheai-config` → all pass. `cargo clippy -p entheai-config -- -D warnings` → clean. `cargo fmt -p entheai-config`.

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): [obsidian] wiki-sync config block"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 11: runtime glue + bin wiring — `ObsidianSession` per session

Tie the layers together (`start` → resolve → seed → watch loop) and spawn/stop it from `bin/entheai`, fail-safe and non-blocking.

**Files:**
- Modify: `crates/obsidian/src/lib.rs`
- Modify: `bin/entheai/Cargo.toml`
- Modify: `bin/entheai/src/main.rs`

- [ ] **Step 1: Implement the runtime glue in `lib.rs`**

Add to `crates/obsidian/src/lib.rs` (below the `pub use`/module lines):

```rust
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// Runtime options (bin maps `entheai_config::ObsidianConfig` → this; keeps the
/// obsidian crate free of a config dependency).
#[derive(Debug, Clone)]
pub struct ObsidianOptions {
    pub enabled: bool,
    pub vault_path: String,
    pub subtree: String,
    pub watch: Vec<String>,
    pub debounce_ms: u64,
    pub mcp_nudge: bool,
    pub mcp_port: u16,
    pub include_architecture: bool,
    pub include_sessions: bool,
}

/// A session-scoped sync task. Dropping it aborts the watcher (stop on exit).
pub struct ObsidianSession {
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ObsidianSession {
    fn inert() -> Self {
        Self { task: None }
    }
}

impl Drop for ObsidianSession {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// Start syncing for `repo_root`. Fail-safe: any problem disables the feature
/// for this session and never propagates. Must be called inside a Tokio runtime.
pub fn start(opts: &ObsidianOptions, repo_root: &Path, home: &Path) -> ObsidianSession {
    if !opts.enabled {
        return ObsidianSession::inert();
    }
    let opts = opts.clone();
    let root = repo_root.to_path_buf();
    let home = home.to_path_buf();
    let task = tokio::spawn(async move {
        if let Err(e) = run(opts, root, home).await {
            log::warn!("obsidian: sync disabled this session: {e}");
        }
    });
    ObsidianSession { task: Some(task) }
}

async fn run(opts: ObsidianOptions, root: PathBuf, home: PathBuf) -> anyhow::Result<()> {
    let Some(vault) = resolve::resolve_vault(&root, &opts.vault_path, &home) else {
        log::debug!("obsidian: no vault resolves for {} — sync off", root.display());
        return Ok(());
    };
    let subtree = vault.join(&opts.subtree);
    let mut writer = writer::VaultWriter::new(subtree.clone());

    // Seed once (lazy: apply() creates nothing if the render is empty).
    apply(&opts, &root, &mut writer)?;
    if opts.mcp_nudge {
        let changed: Vec<PathBuf> = writer.last_changed().to_vec();
        nudge::best_effort(opts.mcp_port, &subtree, &changed).await;
    }

    // Watch → re-apply on each debounced batch.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _watcher = watcher::spawn(&root, &opts.watch, Duration::from_millis(opts.debounce_ms), tx)?;
    while rx.recv().await.is_some() {
        if let Err(e) = apply(&opts, &root, &mut writer) {
            log::warn!("obsidian: apply failed: {e}");
            continue;
        }
        if opts.mcp_nudge {
            let changed: Vec<PathBuf> = writer.last_changed().to_vec();
            nudge::best_effort(opts.mcp_port, &subtree, &changed).await;
        }
    }
    Ok(())
}

/// scan → render_all → write. The tested pipeline (via the writer/render tests).
fn apply(opts: &ObsidianOptions, root: &Path, writer: &mut writer::VaultWriter) -> anyhow::Result<()> {
    let ropts = render::RenderOptions {
        include_architecture: opts.include_architecture,
        include_sessions: opts.include_sessions,
    };
    let ctx = scan::scan(root, ropts)?;
    let out = render::render_all(&ctx);
    writer.apply(&out, root)?;
    Ok(())
}
```

- [ ] **Step 2: Add the dep to the bin**

`bin/entheai/Cargo.toml` — add under `[dependencies]` (matching the sibling-path style):

```toml
entheai-obsidian = { path = "../../crates/obsidian" }
```

- [ ] **Step 3: Wire the session into `main`**

`bin/entheai/src/main.rs` is HOT — re-read it now. Add a mapping helper (module-level fn, near `memory_runtime_config`):

```rust
fn obsidian_options(o: &entheai_config::ObsidianConfig) -> entheai_obsidian::ObsidianOptions {
    entheai_obsidian::ObsidianOptions {
        enabled: o.enabled,
        vault_path: o.vault_path.clone(),
        subtree: o.subtree.clone(),
        watch: o.watch.clone(),
        debounce_ms: o.debounce_ms,
        mcp_nudge: o.mcp_nudge,
        mcp_port: o.mcp_port,
        include_architecture: o.include_architecture,
        include_sessions: o.include_sessions,
    }
}
```

In `main`, immediately after `let shared_memory = build_memory(&cfg)?;` / `let session_id = …;` (the memory construction block, ~line 74), start the session and hold the guard for the rest of `main`:

```rust
    // Obsidian wiki-sync: session-scoped, fail-safe, stops on drop at end of main.
    let _obsidian = entheai_obsidian::start(
        &obsidian_options(&cfg.obsidian),
        &root,
        std::path::Path::new(&std::env::var("HOME").unwrap_or_default()),
    );
```

Because `_obsidian` is bound in `main`'s scope before the `match cli.prompt { … }`, it lives through both the one-shot and the interactive (`entheai_tui::run(...).await`) arms, and is dropped (aborting the watcher) when `main` returns or `?`-exits. The `--app` and `--memory` early-returns happen *before* this line, so they never start the watcher.

- [ ] **Step 4: Build + offline smoke**

Run: `cargo build -p entheai-obsidian -p entheai`
Expected: compiles.

Run (proves the guard is inert with no vault — must not hang or error):
```bash
cd /tmp && rm -rf entheai-obsidian-smoke && mkdir entheai-obsidian-smoke && cd entheai-obsidian-smoke
cat > entheai.toml <<'TOML'
default_model = "osaurus/qwen3-coder"
[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
TOML
cargo run -q -p entheai --manifest-path /Users/peter.lodri/workspace/entropy-om/entheai/Cargo.toml -- --config entheai.toml --no-companion --memory stats
```
Expected: `--memory stats` still runs and exits 0 (the obsidian guard is created only on the agent path, not the `--memory` early-return — this just confirms the bin still builds/links and nothing regressed). No `entheai-sync/` folder is created for `/tmp/entheai-obsidian-smoke` (no vault resolves).

- [ ] **Step 5: Full workspace gate + commit**

Run: `./scripts/check.sh`
Expected: fmt clean, clippy `-D warnings` clean, all tests pass.

```bash
git add crates/obsidian/src/lib.rs bin/entheai/Cargo.toml bin/entheai/src/main.rs Cargo.lock
git commit -m "feat(obsidian): runtime glue + per-session bin wiring (fail-safe ObsidianSession)"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Final verification

- [ ] **Workspace gate**

Run: `./scripts/check.sh`
Expected: fmt clean, clippy `-D warnings` clean, **all** workspace tests pass.

- [ ] **Crate coverage sanity**

Run: `cargo test -p entheai-obsidian`
Expected: render/generators/scan/resolve/writer/nudge tests all pass (the `watcher` FS test too, unless it was `#[ignore]`'d).

- [ ] **End-to-end seed smoke (real vault)**

```bash
# Use a temp HOME so we don't touch the real iCloud vault.
TMPHOME=$(mktemp -d)
VAULT="$TMPHOME/Library/Mobile Documents/iCloud~md~obsidian/entheai"
mkdir -p "$VAULT/.obsidian"
# Run entheai in the real repo but with HOME overridden so the temp vault resolves.
cd /Users/peter.lodri/workspace/entropy-om/entheai
HOME="$TMPHOME" cargo run -q -p entheai -- --config entheai.toml --no-companion --memory stats >/dev/null 2>&1 || true
# The --memory path returns before the watcher starts, so drive the agent path briefly instead:
HOME="$TMPHOME" timeout 6 cargo run -q -p entheai -- --config entheai.toml --no-companion "say hi" >/dev/null 2>&1 || true
ls "$VAULT/entheai-sync" 2>/dev/null && echo "SEEDED" || echo "NO SEED (check model/agent path ran)"
```
Expected: `<VAULT>/entheai-sync/` contains `Home.md`, `Architecture.md`, a docs mirror, `Sessions/`, and `.entheai-sync-manifest.json`. (A model error from Osaurus being down is fine — the seed runs at session start, before the model call.) Clean up: `rm -rf "$TMPHOME"`.

- [ ] **Confirm invariants**

- Grep the writer confinement is intact: `grep -n "refusing non-confined" crates/obsidian/src/writer.rs`.
- Confirm nothing outside the subtree was created in the smoke run: only `<VAULT>/entheai-sync/**` changed; `<VAULT>/Home.md` (if any hand-written) untouched.
- Confirm the memory-highlights SEAM comment is present and no memory code was added: `grep -rn "SEAM(v1.1" crates/obsidian/src`.

---

## Notes for the executor

- **Determinism:** the pure layer must be byte-identical across runs for the writer's hash-skip to work. The only time source is `{UPDATED}`, replaced by the writer *after* hashing — never introduce wall-clock/random into `render.rs`/`generators.rs`.
- **Fail-safe is non-negotiable:** every runtime error path logs and disables sync for the session; nothing in this crate may `?`-propagate into the agent loop or panic the process. The watcher runs in a spawned task (panic-isolated).
- **Scoped commits, push immediately.** `crates/obsidian/**` is collision-free; `crates/config/src/lib.rs` and `bin/entheai/src/main.rs` are hot — re-read before editing, rebase on non-FF.
- **Snyk** `snyk_code_scan` is unavailable here — note it, don't block.
- **Out of scope:** two-way sync, launchd daemon, memory-highlights (leave the seam), non-macOS vault paths.
