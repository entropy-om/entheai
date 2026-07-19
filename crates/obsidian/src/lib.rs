//! entheai Obsidian wiki-sync layer. See
//! docs/superpowers/specs/2026-07-19-entheai-obsidian-wiki-sync-design.md.

pub mod generators;
pub mod render;
pub mod resolve;
pub mod scan;
pub mod writer;

pub use render::{
    render_all, AssetRef, CrateInfo, RenderOptions, RenderOutput, RepoContext, SourceDoc, VaultNote,
};

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
