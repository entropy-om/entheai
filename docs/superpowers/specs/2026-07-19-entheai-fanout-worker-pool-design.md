# entheai — Fan-out local worker pool + `entheai-worker` binary

**Design spec** · 2026-07-19 · status: approved — ready for implementation planning

## 1. Summary

This is the first v0.2 slice of entheai's fan-out orchestration + distributed
workers layer (see `docs/superpowers/specs/2026-07-18-entheai-hybrid-coding-agent-design.md`
§4/§5.4/§5.12/§8). Today, `crates/orchestrator::run_fanout` decomposes a task
into a flat list of `{role, task}` sub-tasks and runs coders in parallel via
`stream::iter(...).buffer_unordered(max_par)` — a hung coder blocks the whole
batch (`buffer_unordered` awaits all futures to completion) and there is no
way to observe or cancel an in-flight coder.

This slice replaces that inline dispatch with a tracked, cancellable
**`WorkerPool`** (per-coder timeout + liveness), adds a standalone headless
**`entheai-worker`** binary (the v0.1 target named in the design spec's crate
map, not yet built), and wires a real **`/workers [list, stop, debug]`**
command into the TUI. It does **not** build DAG-ordered sub-tasks, richer
role fallback chains, or remote/Tailscale execution — those stay v0.2/v0.3
per the parent spec's own roadmap split (§8).

## 2. Scope & session boundaries

Touches only: `crates/orchestrator`, `crates/router`, a new `bin/entheai-worker`,
and fan-out wiring in `bin/entheai`/`crates/tui`. Does **not** touch
`crates/memory`, `crates/companion`, `crates/radio`, `public/`/`assets/`, or
the second-brain server — those are owned by concurrent sessions on this
shared `main` checkout. Scoped `git add <exact paths>` only; push immediately
after each commit per the repo's multi-session convention.

## 3. `WorkerPool` — the local executor seam (`crates/orchestrator`)

New module `crates/orchestrator/src/pool.rs`:

```rust
pub struct WorkerPool {
    workers: Mutex<HashMap<WorkerId, WorkerHandle>>,
    semaphore: Semaphore,       // bounds concurrency to router.max_parallel
    next_id: AtomicUsize,
}

#[derive(Debug, Clone)]
pub enum WorkerStatus {
    Queued,
    Running { started_at: Instant },
    Done,
    TimedOut,
    Killed,
}

struct WorkerHandle {
    join: JoinHandle<CoderRun>,
    abort: AbortHandle,
    status: WorkerStatus,
    role: String,
    task: String,
}

pub struct WorkerSummary {
    pub id: WorkerId,
    pub role: String,
    pub task: String,
    pub status: WorkerStatus,
}
```

- `pool.spawn<F>(role, task, timeout, fut: F) -> WorkerId where F: Future<Output = CoderRun> + Send + 'static`.
  Generic over the future (not hardwired to `run_coder`/`Config`) so pool
  mechanics are unit-testable without a real agent/provider — tests inject a
  fake future (e.g. `async { sleep(50ms).await; CoderRun{..} }`).
- `spawn` immediately `tokio::spawn`s a wrapper task (status starts `Queued`);
  the wrapper first acquires a semaphore permit (bounding real concurrent
  work to `router.max_parallel`, flipping status to `Running { started_at }`
  once acquired), then runs `tokio::time::timeout(timeout, fut)`. On timeout
  expiry the wrapper aborts and sets `TimedOut`; on normal completion, `Done`.
- `pool.list() -> Vec<WorkerSummary>` — snapshot of all tracked workers.
- `pool.stop(id: WorkerId) -> bool` — calls the handle's `AbortHandle::abort()`,
  sets status `Killed`; returns `false` if `id` is unknown or already finished.
- `pool.status(id) -> Option<WorkerStatus>`, `pool.output_snapshot(id) -> Option<String>`
  (only populated once a worker reaches `Done`/`TimedOut`/`Killed` — `run_task`
  returns only a final string, no streaming, so a still-`Running` worker has
  no live output tail; callers must not imply one exists).
- New config: `[fanout].coder_timeout_secs` (optional `u64`; a sensible
  default, e.g. `600`, applies when unset — see `entheai_config::FanoutConfig`).

### `run_fanout` integration

`run_fanout(config, root, task, events, pool: Arc<WorkerPool>)` gains a `pool`
parameter. Its dispatch step changes from:

```rust
let runs: Vec<CoderRun> = stream::iter(wts.iter().cloned())
    .map(|(wt, st)| run_coder(config, wt, st, events.clone()))
    .buffer_unordered(max_par)
    .collect()
    .await;
```

to spawning each sub-task through the pool and awaiting the resulting
`WorkerId`s' join handles (via `pool`'s internal bookkeeping), preserving the
existing `FanoutEvent::CoderStarted`/`CoderFinished` emission timing. The
post-run commit/verify loop treats `TimedOut` and `Killed` outcomes as
"no changes" (worktree is never integrated — it may be mid-edit and is
discarded like any other failed/uncommitted coder).

### Shared coder-execution logic (dedup with `entheai-worker`)

`run_coder`'s inner body (build agent via `entheai_router`, construct the
write-registry rooted at the worktree, run the coder messages, capture
errors as output rather than propagating) is extracted into:

```rust
pub async fn run_coder_once(
    config: &Config,
    role: &str,
    task: &str,
    worktree_path: &Path,
) -> CoderRun
```

Both the in-process `WorkerPool` dispatch (via `run_coder`, which now wraps
`run_coder_once` for event emission + `CoderRun` field population) and
`entheai-worker`'s `main()` call this shared function — no duplicated
agent-invocation logic between the two binaries.

## 4. `bin/entheai-worker` — standalone headless binary

New binary crate (Cargo workspace member), portable subset per the parent
spec's §4 crate map. **Not** wired into `run_fanout`'s dispatch this slice —
it is a correct, independently-runnable unit, and the future remote-execution
target once federation (§5.12) lands in v0.3.

```
$ entheai-worker --config path/to/entheai.toml --role coder \
    --task "add a null check" --worktree /path/to/isolated/checkout
```

- Loads `Config` the same way `bin/entheai` does (via `entheai_config`).
- Calls `entheai_orchestrator::run_coder_once(&config, &role, &task, &worktree)`.
- Prints the resulting `CoderRun` as a single JSON object to stdout
  (`role`, `task`, `output`, and whether the sub-agent errored); exits
  non-zero if the sub-agent's output indicates an error (mirrors today's
  `"error: coder failed: {e}"` convention in `run_coder`'s output capture).
- No IPC protocol, no process-pool management, no subprocess spawning from
  `run_fanout` this slice.

## 5. `/workers` in the TUI (`crates/tui/src/lib.rs`)

Follows the existing ad-hoc per-command string-match pattern already used
for `/radio` (`t == "/radio" || t.starts_with("/radio ")`) — **not** the
generic slash-command parser/dispatcher noted separately as its own future
feature (see project memory `entheai-v1-slash-commands`); that stays out of
scope here to avoid overlapping a different pillar's planned work.

- `App` gains `worker_pool: Option<Arc<WorkerPool>>`, set immediately before
  `tokio::spawn(run_fanout(...))` (mirrors how `fanout_rx` is already
  threaded through at the same call site) and cleared to `None` at the same
  place `fanout_rx` is cleared today (the `None` arm when the events sender
  drops / the run finishes).
- `/workers` or `/workers list` — renders each `WorkerSummary`:
  `[id] role "task" — Running 12s` / `Done` / `TimedOut` / `Killed`. If
  `worker_pool` is `None`, prints "no fan-out running."
- `/workers stop <id>` — `pool.stop(id)`; prints `"stopped worker <id>"` or
  `"no such worker"`.
- `/workers debug <id>` — prints role/task/status/elapsed; if finished,
  includes `pool.output_snapshot(id)`; if still `Running`, states plainly
  that no live output tail is available yet.
- One-shot CLI path (`bin/entheai/src/main.rs --fanout`) passes a throwaway
  `Arc::new(WorkerPool::new())` it never queries.

## 6. Testing & acceptance

- `WorkerPool` unit tests (fast, no LLM calls, generic-future injection):
  `Queued → Running → Done` transition; `stop()` mid-run → `Killed`; short
  timeout → `TimedOut`; `list()` reflects concurrent workers bounded by the
  semaphore.
- `run_coder_once` extraction is a refactor-under-test — existing
  orchestrator tests (`parse_decomposition`, `format_v2_report`, worktree
  round-trips) must keep passing unmodified.
- `entheai-worker`: no real-LLM test in CI; acceptance is a manual run against
  a real worktree (per the `verify` skill) confirming JSON output shape and
  non-zero exit on sub-agent error.
- `/workers`: string-match/formatting unit tests in the same style as
  existing `/radio` tests, plus a manual TUI smoke run.
- Gate: `./scripts/check.sh` (fmt --all + clippy --workspace + nextest) stays
  green; `cargo fmt --all -- --check` before every commit.

## 7. Non-goals (explicitly deferred)

- DAG-ordered dependencies between sub-tasks (flat parallel stays, just
  pool-tracked instead of `buffer_unordered`).
- Richer role fallback chains (`docs`/`test`/`review`/`merge` with a real
  fallback list in `model_for_role`) — today's simple config-lookup-or-
  orchestrator fallback is unchanged.
- Remote/Tailscale execution (`Remote(node)` executor variant) — this slice
  only builds the `Local` seam; `entheai-worker` becomes relevant to it later
  without being wired into it now.
- The generic slash-command parser/dispatcher — `/workers` is one more
  ad-hoc branch alongside `/radio`, not a new framework.

## 8. Key decisions & rationale

- **Executor seam stays in-process this slice.** `entheai-worker` is built and
  independently testable, but `run_fanout` keeps dispatching coders in-process
  via `WorkerPool` — avoids inventing an IPC/wire protocol before the v0.3
  remote-execution design is settled, while still producing the shared
  `run_coder_once` logic that protocol will eventually reuse.
- **Pool takes a generic future, not a `SubTask`.** Decouples pool mechanics
  (timeout/abort/status tracking) from agent/provider wiring, making the pool
  itself cheaply unit-testable.
- **Caller constructs and owns the `Arc<WorkerPool>`.** Simpler than routing a
  live handle back through the `FanoutEvent` channel; the TUI already holds
  onto `fanout_rx` the same way, so holding `worker_pool` alongside it is a
  natural extension of an existing pattern.
- **`/workers` follows the `/radio` ad-hoc pattern**, not the planned generic
  command parser — keeps this slice inside its file scope and avoids
  colliding with a different pillar's already-flagged future work.
