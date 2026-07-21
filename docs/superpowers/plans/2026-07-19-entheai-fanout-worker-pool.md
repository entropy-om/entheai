# Fan-out Local Worker Pool + `entheai-worker` Binary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace fan-out's inline `buffer_unordered` coder dispatch with a tracked, cancellable `WorkerPool` (timeout + liveness), add a standalone `entheai-worker` headless binary, and wire a real `/workers list/stop/debug` TUI command.

**Architecture:** A new `WorkerPool` in `crates/orchestrator/src/pool.rs` spawns each coder as a tokio task behind a semaphore (bounding concurrency to `router.max_parallel`), tracks `Queued/Running/Done/TimedOut/Killed` status via `std::sync::Mutex`-guarded bookkeeping, and exposes `list/stop/status/output_snapshot`. `run_fanout` gains an `Arc<WorkerPool>` parameter supplied by its caller (CLI or TUI) and dispatches through it instead of `buffer_unordered`. The agent-invocation logic shared by in-process dispatch and the new `entheai-worker` binary is extracted into `run_coder_once`.

**Tech Stack:** Rust, tokio (`sync`, `time`, `process`, `fs`), existing `entheai-config`/`entheai-router`/`entheai-tools` crates.

**Spec:** `docs/superpowers/specs/2026-07-19-entheai-fanout-worker-pool-design.md`

---

### Task 1: Config — `[fanout].coder_timeout_secs`

**Files:**
- Modify: `crates/config/src/lib.rs:70-77` (`FanoutConfig`)
- Test: `crates/config/src/lib.rs` (existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/config/src/lib.rs` (near `fanout_verify_defaults_to_none`, around line 219-229):

```rust
    #[test]
    fn parses_fanout_coder_timeout_secs_when_present() {
        let cfg = Config::from_toml_str(
            r#"
            [fanout]
            coder_timeout_secs = 120
            "#,
        )
        .unwrap();

        assert_eq!(cfg.fanout.coder_timeout_secs, 120);
    }

    #[test]
    fn fanout_coder_timeout_secs_defaults_to_600() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.fanout.coder_timeout_secs, 600);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p entheai-config parses_fanout_coder_timeout_secs_when_present fanout_coder_timeout_secs_defaults_to_600`
Expected: FAIL with "no field `coder_timeout_secs` on type `FanoutConfig`"

- [ ] **Step 3: Add the field**

Replace `crates/config/src/lib.rs:70-77`:

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FanoutConfig {
    /// Shell command run inside each coder's worktree to decide whether its
    /// changes are integrated (e.g. "cargo test"). Unset = integrate all
    /// changed branches without verifying.
    #[serde(default)]
    pub verify: Option<String>,
}
```

with:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct FanoutConfig {
    /// Shell command run inside each coder's worktree to decide whether its
    /// changes are integrated (e.g. "cargo test"). Unset = integrate all
    /// changed branches without verifying.
    #[serde(default)]
    pub verify: Option<String>,
    /// Per-coder timeout in seconds before it's force-aborted — a hung coder
    /// must not block the rest of the fan-out batch. Default: 600 (10 min).
    #[serde(default = "default_coder_timeout_secs")]
    pub coder_timeout_secs: u64,
}

impl Default for FanoutConfig {
    fn default() -> Self {
        Self {
            verify: None,
            coder_timeout_secs: default_coder_timeout_secs(),
        }
    }
}

fn default_coder_timeout_secs() -> u64 {
    600
}
```

(Switched from `#[derive(Default)]` to a manual `impl Default` — same pattern already used by `RouterConfig`/`CompanionConfig` in this file — so `FanoutConfig::default()` stays consistent with the serde default instead of giving `coder_timeout_secs: 0`.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p entheai-config`
Expected: PASS (all config tests, including the two new ones)

- [ ] **Step 5: Commit**

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): add [fanout].coder_timeout_secs (default 600s)"
git push origin main
```

---

### Task 2: Orchestrator — extract `run_coder_once`

Pure refactor: pulls the agent-invocation body out of `run_coder` into a standalone `pub` function so both the in-process `WorkerPool` dispatch (Task 4) and the new `entheai-worker` binary (Task 5) share one implementation instead of duplicating it. No behavior change — existing tests must keep passing unmodified.

**Files:**
- Modify: `crates/orchestrator/src/lib.rs:290-330` (`run_coder`)

- [ ] **Step 1: Run the existing test suite first, to have a known-green baseline**

Run: `cargo test -p entheai-orchestrator`
Expected: PASS (baseline before the refactor)

- [ ] **Step 2: Extract `run_coder_once` and slim down `run_coder`**

Replace `crates/orchestrator/src/lib.rs:287-330`:

```rust
/// Run one coder sub-agent to completion inside its own worktree. Never returns
/// Err — a failure is captured as the run's `output`, mirroring [`run_subagent`],
/// so one bad coder doesn't sink the whole fan-out.
async fn run_coder(
    config: &Config,
    wt: worktree::Worktree,
    st: SubTask,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
) -> CoderRun {
    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::CoderStarted {
            index: wt.index,
            role: st.role.clone(),
            task: st.task.clone(),
        });
    }
    let output = async {
        let model_id = entheai_router::model_for_role(config, &st.role)?;
        let agent = entheai_router::build_agent(&model_id, config)?;
        let registry = write_registry(&wt.path);
        let policy = yolo();
        let mut prompter = AutoAllow;
        let out = agent
            .run_task(
                coder_messages(&st.role, &st.task),
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await?;
        Ok::<String, anyhow::Error>(out)
    }
    .await
    .unwrap_or_else(|e| format!("error: coder failed: {e}"));
    CoderRun {
        index: wt.index,
        role: st.role,
        task: st.task,
        branch: wt.branch,
        path: wt.path,
        output,
    }
}
```

with:

```rust
/// Run one coder sub-agent to completion against `worktree_path`: resolve its
/// model via the router, build the write-capable tool registry rooted at the
/// worktree, and run it under a yolo policy. Never returns `Err` — a failure
/// is captured as `"error: coder failed: {e}"` text so one bad coder never
/// aborts its caller. Standalone entry point for `entheai-worker`; also used
/// by [`run_coder`] (the in-process, `WorkerPool`-tracked dispatch path).
pub async fn run_coder_once(config: &Config, role: &str, task: &str, worktree_path: &Path) -> String {
    async {
        let model_id = entheai_router::model_for_role(config, role)?;
        let agent = entheai_router::build_agent(&model_id, config)?;
        let registry = write_registry(worktree_path);
        let policy = yolo();
        let mut prompter = AutoAllow;
        let out = agent
            .run_task(
                coder_messages(role, task),
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await?;
        Ok::<String, anyhow::Error>(out)
    }
    .await
    .unwrap_or_else(|e| format!("error: coder failed: {e}"))
}

/// Run one coder sub-agent to completion inside its own worktree. Never returns
/// Err — a failure is captured as the run's `output`, mirroring [`run_subagent`],
/// so one bad coder doesn't sink the whole fan-out.
async fn run_coder(
    config: Arc<Config>,
    wt: worktree::Worktree,
    st: SubTask,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
) -> CoderRun {
    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::CoderStarted {
            index: wt.index,
            role: st.role.clone(),
            task: st.task.clone(),
        });
    }
    let output = run_coder_once(&config, &st.role, &st.task, &wt.path).await;
    CoderRun {
        index: wt.index,
        role: st.role,
        task: st.task,
        branch: wt.branch,
        path: wt.path,
        output,
    }
}
```

Note `run_coder`'s `config` parameter changes from `&Config` to `Arc<Config>` — required so the future it returns is `'static` once it's spawned via `WorkerPool::spawn` in Task 4 (a plain borrowed `&Config` can't outlive the caller's stack frame inside a `tokio::spawn`'d task). `&config` at the `run_coder_once(&config, ...)` call site relies on `Arc<Config>`'s `Deref` coercion to `&Config` — standard, no explicit deref needed.

Also add near the top of `crates/orchestrator/src/lib.rs` (with the other `use` statements, currently lines 17-22):

```rust
use std::sync::Arc;
use std::time::Duration;
```

- [ ] **Step 3: Run the test suite again to confirm nothing broke**

Run: `cargo test -p entheai-orchestrator`
Expected: PASS — same tests as Step 1's baseline (this step doesn't compile clean yet in isolation, because `run_coder`'s only caller, in `run_fanout`, still passes `config: &Config` — that call site is fixed in Task 4. If you want a green build at this exact checkpoint, temporarily also apply Task 4's dispatch-loop edit before running tests; otherwise proceed straight to Task 3 (`pool.rs`) and come back to Task 4 to make the crate compile again before testing.)

- [ ] **Step 4: Commit**

```bash
git add crates/orchestrator/src/lib.rs
git commit -m "refactor(orchestrator): extract run_coder_once, shared by future entheai-worker binary"
git push origin main
```

---

### Task 3: Orchestrator — `WorkerPool` (`pool.rs`)

**Files:**
- Modify: `crates/orchestrator/Cargo.toml` (tokio features)
- Create: `crates/orchestrator/src/pool.rs`
- Modify: `crates/orchestrator/src/lib.rs` (register + re-export the module)

- [ ] **Step 1: Add the tokio features `pool.rs` needs**

Replace `crates/orchestrator/Cargo.toml:14`:

```toml
tokio = { workspace = true, features = ["process", "fs"] }
```

with:

```toml
tokio = { workspace = true, features = ["process", "fs", "sync", "time"] }
```

And replace `crates/orchestrator/Cargo.toml:23`:

```toml
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

with:

```toml
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }
```

(dev-dependencies need `"time"` too — the pool tests use `tokio::time::sleep`.)

- [ ] **Step 2: Register the module in `lib.rs`**

Replace `crates/orchestrator/src/lib.rs:24` (`pub mod worktree;`) with:

```rust
pub mod pool;
pub mod worktree;

pub use pool::{WorkerId, WorkerPool, WorkerStatus, WorkerSummary};
```

- [ ] **Step 3: Write the failing tests for spawn → Done → join/list/output_snapshot**

Create `crates/orchestrator/src/pool.rs` with just the skeleton types (no logic yet) plus this first test:

```rust
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
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p entheai-orchestrator spawn_transitions_to_done_and_join_returns_the_run`
Expected: FAIL to compile — "no method named `spawn` found for struct `Arc<WorkerPool>`" (and `join`/`status`/`output_snapshot` likewise missing)

- [ ] **Step 5: Implement `spawn`, `join`, `status`, `output_snapshot`**

Add to `crates/orchestrator/src/pool.rs`, inside `impl WorkerPool` (after the `new` function):

```rust
    /// Spawn `fut` as a tracked worker; returns immediately with its id. The
    /// wrapper task acquires a semaphore permit before running `fut` (flipping
    /// `Queued` -> `Running`), enforces `timeout` via `tokio::time::timeout`
    /// (flipping to `TimedOut` and dropping `fut` on expiry), and records the
    /// completed output for `output_snapshot` on normal completion.
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
        self.workers.lock().unwrap().get(&id).map(|h| h.status.clone())
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
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p entheai-orchestrator spawn_transitions_to_done_and_join_returns_the_run`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/orchestrator/Cargo.toml crates/orchestrator/src/lib.rs crates/orchestrator/src/pool.rs
git commit -m "feat(orchestrator): WorkerPool spawn/join/status/output_snapshot"
git push origin main
```

- [ ] **Step 8: Write the failing test for `list`**

Add to `pool.rs`'s `tests` module:

```rust
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
```

- [ ] **Step 9: Run it to verify it passes immediately**

Run: `cargo test -p entheai-orchestrator list_reports_role_and_task_for_a_tracked_worker`
Expected: PASS (`list` was already implemented in Step 5 alongside the others — this step exists to lock in test coverage for it, not to drive new implementation)

- [ ] **Step 10: Write the failing tests for `stop`**

Add to `pool.rs`'s `tests` module:

```rust
    #[tokio::test]
    async fn stop_aborts_a_running_worker_and_marks_it_killed() {
        let pool = WorkerPool::new(4);
        let id = pool.spawn("coder", "slow task", Duration::from_secs(30), async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            fake_run(0, "coder", "slow task", "should never get here")
        });

        // Give the wrapper task a moment to acquire the semaphore and flip to Running.
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
```

- [ ] **Step 11: Run them to verify they pass**

Run: `cargo test -p entheai-orchestrator stop_aborts_a_running_worker_and_marks_it_killed stop_on_unknown_id_returns_false`
Expected: PASS (`stop` was implemented in Step 5; same rationale as Step 9 — these lock in coverage)

- [ ] **Step 12: Write the failing test for timeout**

Add to `pool.rs`'s `tests` module:

```rust
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
```

- [ ] **Step 13: Run it to verify it passes**

Run: `cargo test -p entheai-orchestrator a_worker_that_outlives_its_timeout_is_marked_timed_out`
Expected: PASS

- [ ] **Step 14: Write the failing test for semaphore-bounded concurrency**

Add to `pool.rs`'s `tests` module:

```rust
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

        // Right after spawning both, b must still be Queued (waiting on the
        // semaphore) while a is Running — the pool bounds real concurrency to
        // max_parallel, not just the number of tracked workers.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(matches!(pool.status(id_a), Some(WorkerStatus::Running { .. })));
        assert!(matches!(pool.status(id_b), Some(WorkerStatus::Queued)));

        pool.join(id_a).await;
        pool.join(id_b).await;
    }
```

- [ ] **Step 15: Run it to verify it passes**

Run: `cargo test -p entheai-orchestrator semaphore_bounds_concurrent_running_workers`
Expected: PASS

(This test is timing-sensitive by nature — it asserts an intermediate concurrency state via a short sleep. If it's ever flaky in CI, widen the 10ms/50ms margins rather than deleting the coverage; there's no lock-step alternative for observing "still queued behind a held permit" without an explicit synchronization hook, which would be overkill for this.)

- [ ] **Step 16: Run the full pool test file + commit**

Run: `cargo test -p entheai-orchestrator pool::`
Expected: PASS (all 6 pool tests)

```bash
git add crates/orchestrator/src/pool.rs
git commit -m "test(orchestrator): WorkerPool list/stop/timeout/semaphore coverage"
git push origin main
```

---

### Task 4: Orchestrator — wire `run_fanout` to the pool

**Files:**
- Modify: `crates/orchestrator/src/lib.rs` (`run_fanout` signature + dispatch loop)

- [ ] **Step 1: Change `run_fanout`'s signature**

Replace `crates/orchestrator/src/lib.rs:403-408`:

```rust
pub async fn run_fanout(
    config: &Config,
    root: &Path,
    task: &str,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
) -> anyhow::Result<String> {
```

with:

```rust
pub async fn run_fanout(
    config: &Config,
    root: &Path,
    task: &str,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    pool: Arc<WorkerPool>,
) -> anyhow::Result<String> {
```

- [ ] **Step 2: Replace the coder-dispatch section**

Replace `crates/orchestrator/src/lib.rs:449-458`:

```rust
    // 3. Run coders in parallel, bounded by max_parallel.
    let mut runs: Vec<CoderRun> = stream::iter(wts.iter().cloned())
        .map(|(wt, st)| {
            let events = events.clone();
            run_coder(config, wt, st, events)
        })
        .buffer_unordered(max_par)
        .collect()
        .await;
    runs.sort_by_key(|r| r.index); // buffer_unordered finishes out of order
```

with:

```rust
    // 3. Dispatch coders through the WorkerPool (tracked, cancellable,
    // timeout-bounded) and collect their outcomes in the same order they were
    // spawned — no re-sort needed here, unlike the old buffer_unordered dispatch.
    let coder_timeout = Duration::from_secs(config.fanout.coder_timeout_secs);
    let config_arc = Arc::new(config.clone());
    let mut worker_ids: Vec<(WorkerId, worktree::Worktree, SubTask)> =
        Vec::with_capacity(wts.len());
    for (wt, st) in wts.iter().cloned() {
        let id = pool.spawn(
            st.role.clone(),
            st.task.clone(),
            coder_timeout,
            run_coder(Arc::clone(&config_arc), wt.clone(), st.clone(), events.clone()),
        );
        worker_ids.push((id, wt, st));
    }

    let mut runs: Vec<CoderRun> = Vec::with_capacity(worker_ids.len());
    for (id, wt, st) in worker_ids {
        let run = match pool.join(id).await {
            Some(run) => run,
            None => {
                let reason = match pool.status(id) {
                    Some(WorkerStatus::Killed) => "coder killed (stopped via /workers)",
                    _ => "coder timed out",
                };
                CoderRun {
                    index: wt.index,
                    role: st.role,
                    task: st.task,
                    branch: wt.branch,
                    path: wt.path,
                    output: format!("error: {reason}"),
                }
            }
        };
        runs.push(run);
    }
```

(The synthesized `CoderRun` for a timed-out/killed worker flows into the *existing* Step 4 commit/verify loop unchanged — it's just another `CoderRun` with error-text output, so `commit_all`/`verify_worktree`/`FanoutEvent::CoderFinished` all handle it exactly as they already handle any other coder. No further changes needed downstream in `run_fanout`.)

- [ ] **Step 3: Run the orchestrator test suite**

Run: `cargo test -p entheai-orchestrator`
Expected: PASS — this also validates Task 2's refactor now that the crate compiles end-to-end again

- [ ] **Step 4: Commit**

```bash
git add crates/orchestrator/src/lib.rs
git commit -m "feat(orchestrator): run_fanout dispatches coders through WorkerPool"
git push origin main
```

---

### Task 5: New `bin/entheai-worker` binary

**Files:**
- Modify: `Cargo.toml` (root workspace members)
- Create: `bin/entheai-worker/Cargo.toml`
- Create: `bin/entheai-worker/src/main.rs`

- [ ] **Step 1: Add the crate to the workspace**

Replace `Cargo.toml:3` (root):

```toml
members = ["crates/config", "crates/providers", "crates/core", "crates/tools", "crates/permission", "crates/tui", "crates/memory", "crates/radio", "crates/companion", "crates/router", "crates/orchestrator", "crates/mcp", "crates/skills", "bin/entheai"]
```

with:

```toml
members = ["crates/config", "crates/providers", "crates/core", "crates/tools", "crates/permission", "crates/tui", "crates/memory", "crates/radio", "crates/companion", "crates/router", "crates/orchestrator", "crates/mcp", "crates/skills", "bin/entheai", "bin/entheai-worker"]
```

- [ ] **Step 2: Create the crate manifest**

Create `bin/entheai-worker/Cargo.toml`:

```toml
[package]
name = "entheai-worker"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
entheai-config = { path = "../../crates/config" }
entheai-orchestrator = { path = "../../crates/orchestrator" }
tokio = { workspace = true }
anyhow.workspace = true
clap.workspace = true
serde_json.workspace = true
```

- [ ] **Step 3: Write the failing tests for the pure helpers**

Create `bin/entheai-worker/src/main.rs` with just the testable helpers + their tests (no `main` yet):

```rust
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use entheai_config::Config;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path to the entheai TOML config (same format as the main binary's).
    #[arg(long, default_value = "entheai.toml")]
    config: String,
    /// The sub-agent role (routes to a model via `[agents.<role>]`).
    #[arg(long)]
    role: String,
    /// The sub-agent's task description.
    #[arg(long)]
    task: String,
    /// Path to the isolated git worktree this coder should run against.
    #[arg(long)]
    worktree: PathBuf,
}

/// Whether `output` (a coder's captured result text) indicates the sub-agent
/// failed, mirroring `entheai_orchestrator::run_coder_once`'s error-capture
/// convention (`"error: coder failed: {e}"`).
fn is_error_output(output: &str) -> bool {
    output.starts_with("error:")
}

/// Render a coder's result as the single JSON line printed to stdout.
fn render_result(role: &str, task: &str, output: &str) -> String {
    serde_json::json!({ "role": role, "task": task, "output": output }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_error_output_detects_the_capture_convention() {
        assert!(is_error_output("error: coder failed: boom"));
        assert!(!is_error_output("added a null check"));
    }

    #[test]
    fn render_result_produces_valid_json_with_expected_fields() {
        let json = render_result("coder", "add x", "did the thing");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["role"], "coder");
        assert_eq!(parsed["task"], "add x");
        assert_eq!(parsed["output"], "did the thing");
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p entheai-worker`
Expected: FAIL — the crate doesn't have a `main` function yet, so `cargo test` will error that a binary crate needs a `main` (or, once one is added, the tests themselves would pass trivially since the helpers are already implemented above — see note below).

Note: unlike the other tasks, the helpers here have no failing-behavior step because they're pure and trivial; the meaningful "red" state is the missing `main` function required for this to be a valid binary crate at all. Step 5 adds it.

- [ ] **Step 5: Add `main`**

Append to `bin/entheai-worker/src/main.rs` (before the `#[cfg(test)]` module):

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg_text = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("reading config {}", cli.config))?;
    let config = Config::from_toml_str(&cfg_text)?;

    let output =
        entheai_orchestrator::run_coder_once(&config, &cli.role, &cli.task, &cli.worktree).await;
    println!("{}", render_result(&cli.role, &cli.task, &output));
    if is_error_output(&output) {
        std::process::exit(1);
    }
    Ok(())
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p entheai-worker`
Expected: PASS (both helper tests)

- [ ] **Step 7: Build it and sanity-check the CLI surface**

Run: `cargo build -p entheai-worker && ./target/debug/entheai-worker --help`
Expected: prints usage showing `--config`, `--role`, `--task`, `--worktree`. (A real run against a live repo + config, per this repo's `verify` skill, is the acceptance step for this binary — no live-LLM test belongs in CI.)

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml bin/entheai-worker
git commit -m "feat(worker): new entheai-worker headless binary (standalone, not yet dispatched-to)"
git push origin main
```

---

### Task 6: Wire the one-shot CLI path (`bin/entheai`)

**Files:**
- Modify: `bin/entheai/src/main.rs:62-64`

- [ ] **Step 1: Construct a pool and pass it to `run_fanout`**

Replace `bin/entheai/src/main.rs:62-64`:

```rust
            if cli.fanout {
                let answer = entheai_orchestrator::run_fanout(&cfg, &root, &prompt, None).await?;
                println!("{answer}");
```

with:

```rust
            if cli.fanout {
                let pool = entheai_orchestrator::WorkerPool::new(cfg.router.max_parallel.max(1));
                let answer =
                    entheai_orchestrator::run_fanout(&cfg, &root, &prompt, None, pool).await?;
                println!("{answer}");
```

- [ ] **Step 2: Build the whole workspace to confirm it compiles**

Run: `cargo build --workspace`
Expected: builds clean (this is also the first point where Tasks 2/3/4's changes are exercised together with a real caller)

- [ ] **Step 3: Commit**

```bash
git add bin/entheai/src/main.rs
git commit -m "feat(cli): --fanout constructs a WorkerPool for run_fanout"
git push origin main
```

---

### Task 7: TUI — `/workers list/stop/debug`

**Files:**
- Modify: `crates/tui/src/lib.rs` (`App` struct, fan-out spawn site, command dispatch, new helpers + tests)

- [ ] **Step 1: Add `worker_pool` to `App`**

Replace `crates/tui/src/lib.rs:183-225`'s field list — insert after the `fanout: bool,` field (line 207) and its doc comment:

```rust
    /// Whether this session runs submitted messages through fan-out
    /// (decompose → parallel coders → integrate) instead of the single-agent
    /// `run_task` loop. Set once at construction; shown in the status bar.
    fanout: bool,
```

with:

```rust
    /// Whether this session runs submitted messages through fan-out
    /// (decompose → parallel coders → integrate) instead of the single-agent
    /// `run_task` loop. Set once at construction; shown in the status bar.
    fanout: bool,
    /// The `WorkerPool` backing the in-flight fan-out run, if any — set right
    /// before spawning `run_fanout`, cleared when that run finishes (same
    /// lifecycle as `fanout_rx`). `/workers list/stop/debug` read/mutate this.
    worker_pool: Option<Arc<entheai_orchestrator::WorkerPool>>,
```

- [ ] **Step 2: Initialize the field**

Replace `crates/tui/src/lib.rs:333` (inside the `App { ... }` literal in `event_loop`):

```rust
        fanout,
```

with:

```rust
        fanout,
        worker_pool: None,
```

- [ ] **Step 3: Construct the pool at fan-out spawn time and clear it when the run finishes**

Replace `crates/tui/src/lib.rs:407-419`:

```rust
                            if fanout {
                                let config = Arc::clone(&config);
                                let root = root.clone();
                                let result_tx = result_tx.clone();
                                let (ftx, frx) =
                                    mpsc::unbounded_channel::<entheai_orchestrator::FanoutEvent>();
                                fanout_rx = Some(frx);
                                tokio::spawn(async move {
                                    let res =
                                        entheai_orchestrator::run_fanout(&config, &root, &text, Some(ftx))
                                            .await;
                                    let _ = result_tx.send(res.map_err(|e| e.to_string())).await;
                                });
                            } else {
```

with:

```rust
                            if fanout {
                                let pool = entheai_orchestrator::WorkerPool::new(
                                    config.router.max_parallel.max(1),
                                );
                                app.worker_pool = Some(Arc::clone(&pool));
                                let config = Arc::clone(&config);
                                let root = root.clone();
                                let result_tx = result_tx.clone();
                                let (ftx, frx) =
                                    mpsc::unbounded_channel::<entheai_orchestrator::FanoutEvent>();
                                fanout_rx = Some(frx);
                                tokio::spawn(async move {
                                    let res =
                                        entheai_orchestrator::run_fanout(&config, &root, &text, Some(ftx), pool)
                                            .await;
                                    let _ = result_tx.send(res.map_err(|e| e.to_string())).await;
                                });
                            } else {
```

Replace `crates/tui/src/lib.rs:472-473` (inside the `Some(result) = result_rx.recv()` arm):

```rust
                events_rx = None;
                fanout_rx = None;
```

with:

```rust
                events_rx = None;
                fanout_rx = None;
                app.worker_pool = None;
```

Replace `crates/tui/src/lib.rs:599` (inside the `maybe_fanout` arm's match):

```rust
                    None => fanout_rx = None, // sender dropped -> run finished
```

with:

```rust
                    None => {
                        fanout_rx = None; // sender dropped -> run finished
                        app.worker_pool = None;
                    }
```

- [ ] **Step 4: Write the failing tests for command detection + status formatting**

Add near the existing `radio_command_detection` test (around `crates/tui/src/lib.rs:1177-1184`):

```rust
    #[test]
    fn workers_command_detection() {
        assert!(is_workers_command("/workers"));
        assert!(is_workers_command("/workers list"));
        assert!(is_workers_command("  /workers stop 0"));
        assert!(!is_workers_command("/workersomething"));
        assert!(!is_workers_command("list workers"));
    }

    #[test]
    fn format_status_describes_each_variant() {
        use entheai_orchestrator::WorkerStatus;
        assert_eq!(format_status(&WorkerStatus::Queued), "queued");
        assert!(format_status(&WorkerStatus::Running {
            started_at: std::time::Instant::now(),
        })
        .starts_with("running "));
        assert_eq!(format_status(&WorkerStatus::Done), "done");
        assert_eq!(format_status(&WorkerStatus::TimedOut), "timed out");
        assert_eq!(format_status(&WorkerStatus::Killed), "killed");
    }

    #[test]
    fn workers_command_reports_no_fanout_running_when_pool_is_none() {
        let mut app = App {
            messages: Vec::new(),
            input: String::new(),
            status: Status::Idle,
            scroll: 0,
            follow: true,
            model_label: "m".into(),
            pending_permission: None,
            run_started: None,
            spinner_frame: 0,
            current_action: String::new(),
            now_playing: None,
            fanout: true,
            worker_pool: None,
            system_prompt: None,
            streaming_idx: None,
            out_tokens: 0,
            verb_idx: 0,
            plan: Vec::new(),
        };
        handle_workers_command(&mut app, "/workers list");
        assert!(app
            .messages
            .last()
            .expect("feedback message")
            .text
            .contains("no fan-out running"));
    }
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `cargo test -p entheai-tui workers_command_detection format_status_describes_each_variant workers_command_reports_no_fanout_running_when_pool_is_none`
Expected: FAIL to compile — `is_workers_command`, `format_status`, `handle_workers_command` don't exist yet; `App` literal is also missing `worker_pool` in the *other* existing test at line ~1188 (`radio_events_update_now_playing`), which will now fail to compile too.

- [ ] **Step 6: Fix the pre-existing test's `App` literal**

Replace `crates/tui/src/lib.rs:1200` (inside `radio_events_update_now_playing`'s `App { ... }` literal):

```rust
            fanout: false,
```

with:

```rust
            fanout: false,
            worker_pool: None,
```

- [ ] **Step 7: Implement `is_workers_command`, `format_status`, `handle_workers_command`**

Add near `is_radio_command`/`handle_radio_command` (after `crates/tui/src/lib.rs:761`, right after `handle_radio_command`'s closing brace):

```rust
/// True when the submitted input is a local `/workers` command (never sent to
/// the agent).
fn is_workers_command(text: &str) -> bool {
    let t = text.trim_start();
    t == "/workers" || t.starts_with("/workers ")
}

/// Human-readable rendering of a worker's status for `/workers list`/`debug`.
fn format_status(status: &entheai_orchestrator::WorkerStatus) -> String {
    use entheai_orchestrator::WorkerStatus;
    match status {
        WorkerStatus::Queued => "queued".to_string(),
        WorkerStatus::Running { started_at } => {
            format!("running {}s", started_at.elapsed().as_secs())
        }
        WorkerStatus::Done => "done".to_string(),
        WorkerStatus::TimedOut => "timed out".to_string(),
        WorkerStatus::Killed => "killed".to_string(),
    }
}

/// Parse and dispatch a `/workers [list, stop <id>, debug <id>]` command
/// against the in-flight fan-out's `WorkerPool` (if any), echoing feedback
/// into the history.
///
/// Forms: `/workers` / `/workers list` · `/workers stop <id>` · `/workers debug <id>`.
fn handle_workers_command(app: &mut App, text: &str) {
    app.messages.push(Msg {
        role: Role::User,
        text: text.to_string(),
    });
    let mut parts = text.split_whitespace().skip(1); // skip "/workers"
    let feedback = match &app.worker_pool {
        None => "no fan-out running".to_string(),
        Some(pool) => match (parts.next(), parts.next()) {
            (None, None) | (Some("list"), None) => {
                let mut summaries = pool.list();
                if summaries.is_empty() {
                    "no workers".to_string()
                } else {
                    summaries.sort_by_key(|s| s.id);
                    summaries
                        .iter()
                        .map(|s| {
                            format!(
                                "[{}] {} \"{}\" — {}",
                                s.id,
                                s.role,
                                s.task,
                                format_status(&s.status)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }
            (Some("stop"), Some(id_str)) => match id_str.parse::<entheai_orchestrator::WorkerId>() {
                Ok(id) if pool.stop(id) => format!("stopped worker {id}"),
                Ok(id) => format!("no such worker {id}"),
                Err(_) => format!("invalid worker id: {id_str}"),
            },
            (Some("debug"), Some(id_str)) => match id_str.parse::<entheai_orchestrator::WorkerId>() {
                Ok(id) => match pool.status(id) {
                    None => format!("no such worker {id}"),
                    Some(status) => match pool.output_snapshot(id) {
                        Some(out) => format!("[{id}] {}\n{out}", format_status(&status)),
                        None => format!(
                            "[{id}] {} — still running, no live output tail yet",
                            format_status(&status)
                        ),
                    },
                },
                Err(_) => format!("invalid worker id: {id_str}"),
            },
            _ => "usage: /workers [list | stop <id> | debug <id>]".to_string(),
        },
    };
    app.messages.push(Msg {
        role: Role::Tool,
        text: feedback,
    });
    app.follow = true;
}
```

- [ ] **Step 8: Dispatch `/workers` in the input handler**

Replace `crates/tui/src/lib.rs:391-393`:

```rust
                        Action::Submit(text) if is_radio_command(&text) => {
                            handle_radio_command(&mut app, &radio, &text);
                        }
```

with:

```rust
                        Action::Submit(text) if is_radio_command(&text) => {
                            handle_radio_command(&mut app, &radio, &text);
                        }
                        Action::Submit(text) if is_workers_command(&text) => {
                            handle_workers_command(&mut app, &text);
                        }
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test -p entheai-tui`
Expected: PASS (all tui tests, including the 3 new ones and the fixed pre-existing one)

- [ ] **Step 10: Build the workspace**

Run: `cargo build --workspace`
Expected: builds clean

- [ ] **Step 11: Commit**

```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): /workers list/stop/debug against the in-flight fan-out's WorkerPool"
git push origin main
```

---

### Task 8: Full workspace gate + manual verification

**Files:** none (verification only)

- [ ] **Step 1: Format**

Run: `cargo fmt --all -- --check`
Expected: no diff. If it reports one, run `cargo fmt --all`, review the diff, and fold it into the next commit.

- [ ] **Step 2: Full gate**

Run: `./scripts/check.sh`
Expected: fmt + clippy (workspace) + nextest all green.

- [ ] **Step 3: Manual `entheai-worker` smoke run (per the `verify` skill)**

In a scratch git repo (not this checkout):

```bash
cd /tmp && rm -rf worker-smoke && mkdir worker-smoke && cd worker-smoke
git init -q && git commit --allow-empty -q -m init
cp /Users/peter.lodri/workspace/entropy-om/entheai/entheai.toml . 2>/dev/null || true
/Users/peter.lodri/workspace/entropy-om/entheai/target/debug/entheai-worker \
  --config entheai.toml --role coder --task "create a file called hello.txt with the word hi in it" --worktree .
cat hello.txt
```

Expected: prints a JSON line (`{"role":"coder","task":"...","output":"..."}`), exit code 0, and `hello.txt` exists with the requested content. (If `entheai.toml` isn't present in this checkout, construct a minimal one pointing at a configured provider before running this step — see `bin/entheai/README` or an existing `entheai.toml` for the `[providers.*]`/`[agents.coder]` shape.)

- [ ] **Step 4: Manual TUI smoke run for `/workers`**

Run: `cargo run -p entheai -- --fanout` (from the entheai repo root), submit a real task, then type `/workers list` while it's running, then `/workers debug 0`, then let it finish and type `/workers list` again.
Expected: while running, `list` shows the in-flight coder(s) with a `running Ns` status; after it finishes, `list` reports "no fan-out running" (pool cleared).

- [ ] **Step 5: Fix anything the gate or manual runs surface, then final commit if needed**

```bash
git add -A -- crates/orchestrator crates/router bin/entheai-worker bin/entheai crates/tui
git commit -m "fix: address check.sh / manual verification findings"
git push origin main
```

(Only commit if Steps 1-4 actually required changes — skip this step if everything was already green.)

---

## Self-review notes

- **Spec coverage:** §3 (`WorkerPool`) → Tasks 3-4; §4 (`entheai-worker`) → Task 5; §5 (`/workers` TUI) → Task 7; §6 (testing/acceptance) → the TDD steps throughout + Task 8; §7 (non-goals) → nothing in this plan touches DAG deps, role fallback chains, remote execution, or a generic command parser, confirming scope stayed within it.
- **Type consistency:** `WorkerId = usize` used consistently (config parses `usize`-compatible ids via `.parse::<entheai_orchestrator::WorkerId>()` in Task 7); `WorkerStatus` variants (`Queued`/`Running{started_at}`/`Done`/`TimedOut`/`Killed`) match between Task 3's definition, Task 4's `match pool.status(id)`, and Task 7's `format_status`. `run_coder_once`'s signature (`&Config, &str, &str, &Path`) matches both its Task 2 definition and Task 5's call site in `entheai-worker`'s `main`.
- **Task-2/Task-4 ordering note:** flagged explicitly in Task 2 Step 3 — the crate doesn't fully compile again until Task 4's dispatch-loop edit lands, since `run_coder`'s only caller changes its argument type there. This is intentional (keeps each task's diff focused on one concern) rather than an oversight.
