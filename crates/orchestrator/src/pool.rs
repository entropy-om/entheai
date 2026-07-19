//! Tracked, cancellable local execution for fan-out coders.
//!
//! `WorkerPool` wraps `tokio::spawn` with three things `buffer_unordered`
//! doesn't give you: a **status** you can query mid-run (`Queued` /
//! `Running` / `Done` / `TimedOut` / `Killed`), a **stop()` you can call from
//! outside the batch (e.g. the TUI's `/workers stop`), and a per-worker
//! **timeout** so one hung coder can't block the rest of the fan-out. It is
//! generic only over the future's *output type* (`CoderRun`, fixed) — not
//! over `Config`/agents/worktrees — so its mechanics are unit-testable with
//! plain fake futures, no LLM calls.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::task::{AbortHandle, JoinHandle};

pub type WorkerId = usize;

/// A tracked worker's lifecycle state.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerStatus {
    /// Spawned, waiting for a concurrency-limiting semaphore permit.
    Queued,
    /// Holding a permit and running its future.
    Running { started_at: Instant },
    /// Finished within its timeout.
    Done,
    /// Aborted after exceeding its timeout.
    TimedOut,
    /// Aborted via `WorkerPool::stop`.
    Killed,
}

/// A snapshot of one tracked worker, returned by `WorkerPool::list`.
#[derive(Debug, Clone)]
pub struct WorkerSummary {
    pub id: WorkerId,
    pub role: String,
    pub task: String,
    pub status: WorkerStatus,
}

struct WorkerHandle {
    /// Taken (set to `None`) the first time `join` is called on this id.
    join: Option<JoinHandle<Option<crate::CoderRun>>>,
    abort: AbortHandle,
    status: WorkerStatus,
    role: String,
    task: String,
}

/// A pool of tracked, semaphore-bounded, cancellable local coder executions.
pub struct WorkerPool {
    workers: Mutex<HashMap<WorkerId, WorkerHandle>>,
    outputs: Mutex<HashMap<WorkerId, String>>,
    semaphore: Arc<tokio::sync::Semaphore>,
    next_id: AtomicUsize,
}

impl WorkerPool {
    /// `max_parallel` bounds how many spawned workers may hold the semaphore
    /// (i.e. actually run, as opposed to sit `Queued`) at once.
    pub fn new(max_parallel: usize) -> Arc<Self> {
        Arc::new(Self {
            workers: Mutex::new(HashMap::new()),
            outputs: Mutex::new(HashMap::new()),
            semaphore: Arc::new(tokio::sync::Semaphore::new(max_parallel.max(1))),
            next_id: AtomicUsize::new(0),
        })
    }

    /// Spawn `fut` as a tracked worker; returns immediately with its id. The
    /// wrapper task acquires a semaphore permit before running `fut` (flipping
    /// `Queued` -> `Running`), enforces `timeout` via `tokio::time::timeout`
    /// (flipping to `TimedOut` and dropping `fut` on expiry), and records the
    /// completed output for `output_snapshot` on normal completion.
    ///
    /// `CoderRun` is intentionally crate-private (see the module-level note);
    /// the bound below is only ever satisfied by callers inside this crate,
    /// so the private-in-public-interface lint is a false positive here.
    #[allow(private_bounds)]
    pub fn spawn<F>(
        self: &Arc<Self>,
        role: impl Into<String>,
        task: impl Into<String>,
        timeout: Duration,
        fut: F,
    ) -> WorkerId
    where
        F: Future<Output = crate::CoderRun> + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let role = role.into();
        let task = task.into();
        let pool = Arc::clone(self);
        let semaphore = Arc::clone(&self.semaphore);

        let join_handle: JoinHandle<Option<crate::CoderRun>> = tokio::spawn(async move {
            let _permit = semaphore.acquire_owned().await.expect("semaphore closed");
            pool.mark_running(id);
            match tokio::time::timeout(timeout, fut).await {
                Ok(run) => {
                    pool.mark_done(id, run.output.clone());
                    Some(run)
                }
                Err(_) => {
                    pool.mark_timed_out(id);
                    None
                }
            }
        });

        let abort = join_handle.abort_handle();
        self.workers.lock().unwrap().insert(
            id,
            WorkerHandle {
                join: Some(join_handle),
                abort,
                status: WorkerStatus::Queued,
                role,
                task,
            },
        );
        id
    }

    fn mark_running(&self, id: WorkerId) {
        if let Some(h) = self.workers.lock().unwrap().get_mut(&id) {
            if !matches!(h.status, WorkerStatus::Killed) {
                h.status = WorkerStatus::Running {
                    started_at: Instant::now(),
                };
            }
        }
    }

    fn mark_done(&self, id: WorkerId, output: String) {
        {
            let mut workers = self.workers.lock().unwrap();
            if let Some(h) = workers.get_mut(&id) {
                if !matches!(h.status, WorkerStatus::Killed) {
                    h.status = WorkerStatus::Done;
                }
            }
        }
        self.outputs.lock().unwrap().insert(id, output);
    }

    fn mark_timed_out(&self, id: WorkerId) {
        if let Some(h) = self.workers.lock().unwrap().get_mut(&id) {
            if !matches!(h.status, WorkerStatus::Killed) {
                h.status = WorkerStatus::TimedOut;
            }
        }
    }

    /// Await worker `id`'s outcome. `None` means it timed out or was stopped
    /// (never a real `CoderRun` in that case — the caller decides what a
    /// missing outcome means for its own bookkeeping). Consumes the
    /// underlying join handle the first time it's called for a given `id`;
    /// subsequent calls return `None`.
    #[allow(private_interfaces)]
    pub async fn join(&self, id: WorkerId) -> Option<crate::CoderRun> {
        let handle = self
            .workers
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|h| h.join.take());
        match handle {
            Some(j) => j.await.ok().flatten(),
            None => None,
        }
    }

    /// Snapshot of every tracked worker (regardless of status).
    pub fn list(&self) -> Vec<WorkerSummary> {
        self.workers
            .lock()
            .unwrap()
            .iter()
            .map(|(id, h)| WorkerSummary {
                id: *id,
                role: h.role.clone(),
                task: h.task.clone(),
                status: h.status.clone(),
            })
            .collect()
    }

    /// Current status of `id`, or `None` if it's unknown.
    pub fn status(&self, id: WorkerId) -> Option<WorkerStatus> {
        self.workers
            .lock()
            .unwrap()
            .get(&id)
            .map(|h| h.status.clone())
    }

    /// The captured output text of a *finished* worker (`Done`/`TimedOut` sets
    /// nothing for the latter — there is no output to capture on a timeout).
    /// `None` while still `Queued`/`Running`, or for an unknown id — callers
    /// must not imply a live tail exists for a still-running worker.
    pub fn output_snapshot(&self, id: WorkerId) -> Option<String> {
        self.outputs.lock().unwrap().get(&id).cloned()
    }

    /// Abort `id` if it's still in flight, marking it `Killed`. Returns
    /// `false` if `id` is unknown or already finished.
    pub fn stop(&self, id: WorkerId) -> bool {
        let mut workers = self.workers.lock().unwrap();
        match workers.get_mut(&id) {
            Some(h)
                if !matches!(
                    h.status,
                    WorkerStatus::Done | WorkerStatus::TimedOut | WorkerStatus::Killed
                ) =>
            {
                h.abort.abort();
                h.status = WorkerStatus::Killed;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_run(index: usize, role: &str, task: &str, output: &str) -> crate::CoderRun {
        crate::CoderRun {
            index,
            role: role.to_string(),
            task: task.to_string(),
            branch: format!("entheai/test/coder-{index}"),
            path: std::path::PathBuf::from(format!("/tmp/entheai-test-coder-{index}")),
            output: output.to_string(),
        }
    }

    #[tokio::test]
    async fn spawn_transitions_to_done_and_join_returns_the_run() {
        let pool = WorkerPool::new(4);
        let id = pool.spawn("coder", "add x", Duration::from_secs(5), async {
            fake_run(0, "coder", "add x", "did the thing")
        });

        let run = pool.join(id).await.expect("expected a completed run");
        assert_eq!(run.output, "did the thing");
        assert!(matches!(pool.status(id), Some(WorkerStatus::Done)));
        assert_eq!(pool.output_snapshot(id).as_deref(), Some("did the thing"));
    }

    #[tokio::test]
    async fn list_reports_role_and_task_for_a_tracked_worker() {
        let pool = WorkerPool::new(4);
        let id = pool.spawn("explore", "map auth", Duration::from_secs(5), async {
            fake_run(0, "explore", "map auth", "done")
        });
        pool.join(id).await;

        let summaries = pool.list();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, id);
        assert_eq!(summaries[0].role, "explore");
        assert_eq!(summaries[0].task, "map auth");
    }

    #[tokio::test]
    async fn stop_aborts_a_running_worker_and_marks_it_killed() {
        let pool = WorkerPool::new(4);
        let id = pool.spawn("coder", "slow task", Duration::from_secs(30), async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            fake_run(0, "coder", "slow task", "should never get here")
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(pool.stop(id));
        assert!(matches!(pool.status(id), Some(WorkerStatus::Killed)));
        assert!(pool.join(id).await.is_none());
    }

    #[tokio::test]
    async fn stop_on_unknown_id_returns_false() {
        let pool = WorkerPool::new(4);
        assert!(!pool.stop(999));
    }

    #[tokio::test]
    async fn a_worker_that_outlives_its_timeout_is_marked_timed_out() {
        let pool = WorkerPool::new(4);
        let id = pool.spawn("coder", "hangs", Duration::from_millis(20), async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            fake_run(0, "coder", "hangs", "should never get here")
        });

        let run = pool.join(id).await;
        assert!(run.is_none());
        assert!(matches!(pool.status(id), Some(WorkerStatus::TimedOut)));
    }

    #[tokio::test]
    async fn semaphore_bounds_concurrent_running_workers() {
        let pool = WorkerPool::new(1); // only one may run at a time
        let id_a = pool.spawn("coder", "a", Duration::from_secs(5), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            fake_run(0, "coder", "a", "a done")
        });
        let id_b = pool.spawn("coder", "b", Duration::from_secs(5), async {
            fake_run(1, "coder", "b", "b done")
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(matches!(
            pool.status(id_a),
            Some(WorkerStatus::Running { .. })
        ));
        assert!(matches!(pool.status(id_b), Some(WorkerStatus::Queued)));

        pool.join(id_a).await;
        pool.join(id_b).await;
    }
}
