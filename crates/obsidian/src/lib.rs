//! entheai Obsidian wiki-sync layer. See
//! docs/superpowers/specs/2026-07-19-entheai-obsidian-wiki-sync-design.md.

pub mod generators;
pub mod nudge;
pub mod render;
pub mod resolve;
pub mod scan;
pub mod watcher;
pub mod writer;

pub use render::{
    render_all, AssetRef, CrateInfo, RenderOptions, RenderOutput, RepoContext, SourceDoc, VaultNote,
};

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
        log::debug!(
            "obsidian: no vault resolves for {} — sync off",
            root.display()
        );
        return Ok(());
    };
    let subtree = vault.join(&opts.subtree);
    let mut writer = writer::VaultWriter::new(subtree.clone());
    // Persistent read cache: unchanged files are served from memory across
    // ticks (see `scan::ScanCache`), so a debounced re-scan re-reads only what
    // actually changed instead of the whole repo every tick.
    let mut cache = scan::ScanCache::default();

    // Seed once (lazy: apply() creates nothing if the render is empty). Runs
    // off the runtime: apply() is a blocking scan+render+write pipeline, so it
    // must not execute inline on a Tokio worker thread (P6).
    let opts_c = opts.clone();
    let root_c = root.clone();
    let (w, c, (res, changed)) = tokio::task::spawn_blocking(move || {
        let res = apply(&opts_c, &root_c, &mut writer, &mut cache);
        let changed = writer.last_changed().to_vec();
        (writer, cache, (res, changed))
    })
    .await
    .map_err(|e| anyhow::anyhow!("obsidian apply task panicked: {e}"))?;
    writer = w;
    cache = c;
    res?;
    if opts.mcp_nudge {
        nudge::best_effort(opts.mcp_port, &subtree, &changed).await;
    }

    // Watch → re-apply on each debounced batch.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _watcher = watcher::spawn(
        &root,
        &opts.watch,
        Duration::from_millis(opts.debounce_ms),
        tx,
    )?;
    while rx.recv().await.is_some() {
        let opts_c = opts.clone();
        let root_c = root.clone();
        let (w, c, (res, changed)) = tokio::task::spawn_blocking(move || {
            let res = apply(&opts_c, &root_c, &mut writer, &mut cache);
            let changed = writer.last_changed().to_vec();
            (writer, cache, (res, changed))
        })
        .await
        .map_err(|e| anyhow::anyhow!("obsidian apply task panicked: {e}"))?;
        writer = w;
        cache = c;
        if let Err(e) = res {
            log::warn!("obsidian: apply failed: {e}");
            continue;
        }
        if opts.mcp_nudge {
            nudge::best_effort(opts.mcp_port, &subtree, &changed).await;
        }
    }
    Ok(())
}

/// scan → render_all → write. The tested pipeline (via the writer/render tests).
fn apply(
    opts: &ObsidianOptions,
    root: &Path,
    writer: &mut writer::VaultWriter,
    cache: &mut scan::ScanCache,
) -> anyhow::Result<()> {
    let ropts = render::RenderOptions {
        include_architecture: opts.include_architecture,
        include_sessions: opts.include_sessions,
    };
    let ctx = scan::scan_cached(root, ropts, cache)?;
    let out = render::render_all(&ctx);
    writer.apply(&out, root)?;
    Ok(())
}

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
        let mut debouncer = new_debouncer(Duration::from_millis(200), move |res| {
            let _ = tx.send(res);
        })
        .unwrap();
        debouncer
            .watcher()
            .watch(
                dir.path(),
                notify_debouncer_mini::notify::RecursiveMode::Recursive,
            )
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
