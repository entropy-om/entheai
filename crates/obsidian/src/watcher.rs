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
    Ok(Watcher {
        _debouncer: debouncer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn real_watch_ticks_on_write() {
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
        assert!(
            ticked.is_ok() && ticked.unwrap().is_some(),
            "a write produced a tick"
        );
    }
}
