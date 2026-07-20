//! `CoderExecutor` impl: dispatch a coder sub-task to the worker fleet and
//! squash-apply its delta into the caller's worktree. Any miss (no worker, no
//! result, no change, or an error) returns `None` so `run_fanout` falls back to
//! a local coder. The base bundle is created + uploaded **once per run** (via a
//! `OnceCell`) so concurrent coders don't race on the shared `entheai-fed-base` branch.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::OnceCell;

use crate::{repo, types, Federation, WorkItem};

pub struct FederationExecutor {
    fed: Federation,
    root: PathBuf,
    /// The shared base-bundle object-store key, initialized exactly once.
    base_key: OnceCell<String>,
}

impl FederationExecutor {
    pub fn new(fed: Federation, root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            fed,
            root,
            base_key: OnceCell::new(),
        })
    }

    /// Bundle the repo base (root HEAD == the fan-out base) and upload it, once.
    /// All coders in a run share this bundle, so the (branch-churning) bundle
    /// step happens a single time regardless of concurrency.
    async fn ensure_base_bundle(&self, base_sha: &str) -> Option<String> {
        self.base_key
            .get_or_try_init(|| async {
                let tmp = tempfile::tempdir()?;
                let bundle = tmp.path().join("base.bundle");
                repo::bundle_base(&self.root, &bundle).await?;
                let key = types::base_key(base_sha, 0);
                self.fed
                    .put_bundle(&key, &tokio::fs::read(&bundle).await?)
                    .await?;
                Ok::<_, anyhow::Error>(key)
            })
            .await
            .ok()
            .cloned()
    }
}

async fn git(dir: &Path, args: &[&str]) -> bool {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl entheai_orchestrator::CoderExecutor for FederationExecutor {
    async fn workers_available(&self) -> bool {
        self.fed
            .count_workers(std::time::Duration::from_millis(800))
            .await
            > 0
    }

    async fn execute(
        &self,
        session: &str,
        index: usize,
        base_sha: &str,
        worktree_path: &Path,
        role: &str,
        task: &str,
    ) -> Option<String> {
        let base_key = self.ensure_base_bundle(base_sha).await?;

        // Subscribe BEFORE dispatching so the core-NATS result isn't missed.
        let mut sub = self.fed.subscribe_result(session, index).await.ok()?;
        self.fed
            .dispatch(&WorkItem {
                session: session.into(),
                index,
                role: role.into(),
                task: task.into(),
                base_bundle_key: base_key,
                base_sha: base_sha.into(),
            })
            .await
            .ok()?;

        let result = self.fed.await_result(&mut sub).await?;
        if !result.committed {
            return None; // no-change / error → local fallback
        }

        // Squash-apply the worker's delta into the coder's worktree as
        // UNCOMMITTED changes; run_fanout's commit/verify/integrate does the rest.
        let tmp = tempfile::tempdir().ok()?;
        let rb = tmp.path().join("result.bundle");
        tokio::fs::write(&rb, self.fed.get_bundle(&result.result_bundle_key).await.ok()?)
            .await
            .ok()?;
        // Squash-apply the delta as UNCOMMITTED changes. On ANY failure, restore
        // the worktree to a clean base — otherwise a half-applied/conflicted merge
        // would be left for the local fallback (or `commit_all`) to snapshot and
        // integrate as garbage.
        let applied = git(worktree_path, &["fetch", rb.to_str()?, "fed-work"]).await
            && git(worktree_path, &["merge", "--squash", "FETCH_HEAD"]).await;
        if !applied {
            let _ = git(worktree_path, &["reset", "--hard"]).await;
            let _ = git(worktree_path, &["clean", "-fd"]).await;
            return None;
        }
        Some(result.log)
    }
}
