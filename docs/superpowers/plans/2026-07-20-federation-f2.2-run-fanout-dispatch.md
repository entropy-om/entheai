# Federation F2.2 — Offload fan-out coders to the fleet (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** When `[federation].enabled` **and** at least one worker is serving, `entheai --fanout "…"` runs its coder sub-tasks on the fleet instead of locally — each remote result applied into that coder's worktree so the existing commit/verify/integrate path is unchanged. Per-coder **local fallback** on timeout/no-worker/no-change. Federation off → **byte-identical to today**.

**Architecture:** `orchestrator` stays NATS-agnostic. It defines an object-safe `CoderExecutor` trait; `run_fanout` gains an `Option<Arc<dyn CoderExecutor>>` param. When present and `workers_available()`, each coder is run via the executor, which dispatches a `WorkItem`, awaits the `WorkResult`, and **squash-applies** the worker's delta bundle into the coder's worktree (uncommitted → `commit_all` commits it exactly as a local run would). The `entheai-federation` crate provides the impl (`FederationExecutor`); the bin builds it when enabled and threads it in. Presence: workers heartbeat on `entheai.presence.coder`; the executor counts them in a short window.

**Tech Stack:** Rust (2021, MSRV 1.80). `async-trait` (already a workspace dep). Git via `tokio::process::Command`. Live hub for E2E.

**Key seams (verified):**
- `run_fanout` step 3 (`crates/orchestrator/src/lib.rs:~567`): `pool.spawn(role, task, timeout, run_coder(cfg, wt, st, events))` → each produces `CoderRun { index, role, task, branch, path, output }`; steps 4–5 commit `run.path`/integrate `run.branch`. Applying a remote delta as **uncommitted working-tree changes** in `wt.path` makes those steps behave identically.
- `run_coder(config: Arc<Config>, wt: Worktree, st: SubTask, events) -> CoderRun` (private) — the local path, unchanged.
- `worktree::Worktree { index, branch, path }`; `worktree::resolve_base(root, "HEAD")`.
- F2.1 `entheai_federation`: `Federation::{connect, dispatch, subscribe_result, await_result, put_bundle, get_bundle}`, `WorkItem/WorkResult`, `types::{base_key, result_key}`, `repo::bundle_base`. `FedOptions::from_config(&cfg.nats, &cfg.federation)`.
- `run_fanout` call sites: `bin/entheai/src/main.rs` one-shot (~line 119) and `crates/tui/src/lib.rs` (~line 494).

**Squash-apply mechanism (the crux):** worker returns a bundle of `base_sha..fed-work`. In the coder's worktree (at `base_sha`): `git -C <wt> fetch <bundle> fed-work` → `git -C <wt> merge --squash FETCH_HEAD` stages the delta **without committing**; `commit_all` then commits it onto the coder's branch. No conflict (the worktree is untouched at base).

---

## File Structure
- **Modify `crates/orchestrator/src/lib.rs`** — `CoderExecutor` trait + `run_fanout` executor param + per-coder strategy.
- **Modify `crates/orchestrator/Cargo.toml`** — `async-trait` (already used? add if not).
- **Create `crates/federation/src/executor.rs`** — `FederationExecutor` (impls the trait) + presence count.
- **Modify `crates/federation/src/lib.rs`** — `pub mod executor;`, heartbeat publish helper, presence count.
- **Modify `bin/entheai-worker/src/main.rs`** — `--serve` publishes a presence heartbeat.
- **Modify `bin/entheai/src/main.rs`** + **`crates/tui/src/lib.rs`** — thread the executor param.

---

## Task 1: `CoderExecutor` trait + run_fanout param (behavior identical with `None`)

**Files:** `crates/orchestrator/src/lib.rs`, `crates/orchestrator/Cargo.toml`.

- [ ] **Step 1: Add `async-trait`** to `crates/orchestrator/Cargo.toml` `[dependencies]` if absent: `async-trait = { workspace = true }` (the crate already uses it for `Prompter`; confirm and skip if present).

- [ ] **Step 2: Define the trait** (near the top of `lib.rs`, after the `FanoutEvent` enum):

```rust
/// A strategy for running one coder sub-task. `run_fanout` uses this to
/// optionally offload coders to a remote fleet; `None` = always local (today's
/// behavior). Kept NATS-agnostic — the impl lives in `entheai-federation`.
#[async_trait::async_trait]
pub trait CoderExecutor: Send + Sync {
    /// Cheap check: is at least one worker available right now? When false,
    /// `run_fanout` skips remote dispatch entirely and runs every coder locally.
    async fn workers_available(&self) -> bool;

    /// Run the coder for `(role, task)` remotely, applying its changes into
    /// `worktree_path` as UNCOMMITTED working-tree changes (ready for the normal
    /// commit/verify/integrate path). `base_sha` is the worktree's base commit.
    /// Returns the coder's log on success, or `None` to fall back to local.
    async fn execute(
        &self,
        session: &str,
        index: usize,
        base_sha: &str,
        worktree_path: &std::path::Path,
        role: &str,
        task: &str,
    ) -> Option<String>;
}
```

- [ ] **Step 3: Thread the param through `run_fanout`.** Change the signature (add a trailing param):

```rust
pub async fn run_fanout(
    config: &Config,
    root: &Path,
    task: &str,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    pool: Arc<WorkerPool>,
    executor: Option<Arc<dyn CoderExecutor>>,
) -> anyhow::Result<String> {
```

Just after `let base = worktree::resolve_base(root, "HEAD").await?;` (it already exists ~line 541), decide the strategy once:

```rust
    // Offload coders to the fleet only when an executor is wired AND a worker is
    // actually available; otherwise every coder runs locally (unchanged path).
    let remote = match &executor {
        Some(ex) if ex.workers_available().await => Some((ex.clone(), base.clone())),
        _ => None,
    };
```

- [ ] **Step 4: Use the strategy in step 3's spawn loop.** Replace the `run_coder(...)` argument to `pool.spawn` with a call to a new helper `run_coder_maybe_remote`:

```rust
        let id = pool.spawn(
            st.role.clone(),
            st.task.clone(),
            coder_timeout,
            run_coder_maybe_remote(
                Arc::clone(&config_arc),
                wt.clone(),
                st.clone(),
                events.clone(),
                remote.clone(),
                session.clone(),
            ),
        );
```

Add the helper (near `run_coder`):

```rust
/// Run one coder either remotely (via the executor, applying the delta into the
/// worktree) or locally. On any remote miss (no result / no change / error),
/// falls back to a local `run_coder` so a coder is never silently dropped.
async fn run_coder_maybe_remote(
    config: Arc<Config>,
    wt: worktree::Worktree,
    st: SubTask,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    remote: Option<(Arc<dyn CoderExecutor>, String)>,
    session: String,
) -> CoderRun {
    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::CoderStarted {
            index: wt.index,
            role: st.role.clone(),
            task: st.task.clone(),
        });
    }
    if let Some((ex, base_sha)) = remote {
        if let Some(log) = ex
            .execute(&session, wt.index, &base_sha, &wt.path, &st.role, &st.task)
            .await
        {
            // Remote applied its delta into wt.path; steps 4–5 commit/integrate it.
            return CoderRun {
                index: wt.index,
                role: st.role,
                task: st.task,
                branch: wt.branch,
                path: wt.path,
                output: log,
            };
        }
        log::info!("federation: coder {} fell back to local", wt.index);
    }
    // Local path — identical to before. `run_coder` emits its own CoderStarted,
    // so call the inner runner that does NOT re-emit (see Step 5).
    run_coder_inner(config, wt, st, events).await
}
```

- [ ] **Step 5: Split `run_coder` so the event isn't emitted twice.** `run_coder` currently emits `CoderStarted` then runs. Rename its body to `run_coder_inner` (same code, minus the `CoderStarted` send, which `run_coder_maybe_remote` now owns), and keep a thin `run_coder` wrapper for any other caller that emits + calls inner. (Check callers: only step 3 calls `run_coder` — so you can move the `CoderStarted` emit into `run_coder_maybe_remote` and rename `run_coder`→`run_coder_inner` outright. Verify with `grep -n "run_coder(" crates/orchestrator/src/lib.rs`.)

- [ ] **Step 6: Update the two call sites** to pass `None` (this task keeps behavior identical):
  - `bin/entheai/src/main.rs`: `run_fanout(&cfg, &root, &prompt, events, pool, None)`.
  - `crates/tui/src/lib.rs`: `run_fanout(&config, &root, &text, events, pool, None)`.

- [ ] **Step 7: Verify identical behavior** — `cargo test --workspace` → all green (the orchestrator's existing 33+ fan-out tests must still pass; `None` executor = today's path). `cargo clippy --workspace --all-targets -- -D warnings` clean.

- [ ] **Step 8: Commit**

```bash
git add crates/orchestrator/Cargo.toml crates/orchestrator/src/lib.rs bin/entheai/src/main.rs crates/tui/src/lib.rs
git commit -m "feat(orchestrator): CoderExecutor seam in run_fanout (None = local, unchanged) (F2.2)"
```

---

## Task 2: Presence heartbeat

**Files:** `crates/federation/src/lib.rs`, `bin/entheai-worker/src/main.rs`.

- [ ] **Step 1:** Add to `Federation` (in `lib.rs`):

```rust
const PRESENCE_SUBJECT: &str = "entheai.presence.coder";

impl Federation {
    /// Announce this worker is alive (core NATS, fire-and-forget).
    pub async fn heartbeat(&self) {
        let _ = self.client.publish(PRESENCE_SUBJECT, "1".into()).await;
        let _ = self.client.flush().await;
    }

    /// Count distinct heartbeats seen within `window` (a cheap "any workers?").
    pub async fn count_workers(&self, window: std::time::Duration) -> usize {
        let Ok(mut sub) = self.client.subscribe(PRESENCE_SUBJECT).await else { return 0 };
        // Nudge live workers to answer promptly.
        let _ = self.client.publish("entheai.presence.ping", "?".into()).await;
        let _ = self.client.flush().await;
        let mut n = 0usize;
        let deadline = tokio::time::Instant::now() + window;
        loop {
            match tokio::time::timeout_at(deadline, futures::StreamExt::next(&mut sub)).await {
                Ok(Some(_)) => n += 1,
                _ => break,
            }
        }
        n
    }
}
```

- [ ] **Step 2:** In `entheai-worker --serve` (`run_serve`), spawn a heartbeat task before the claim loop, and also respond to pings:

```rust
    // Heartbeat: announce liveness every 5s, and answer presence pings promptly.
    {
        let fed_hb = fed.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
            loop { ticker.tick().await; fed_hb.heartbeat().await; }
        });
        let fed_ping = fed.clone();
        tokio::spawn(async move {
            if let Ok(mut pings) = fed_ping.subscribe_ping().await {
                while futures::StreamExt::next(&mut pings).await.is_some() {
                    fed_ping.heartbeat().await;
                }
            }
        });
    }
```

Add `subscribe_ping` to `Federation`:
```rust
    pub async fn subscribe_ping(&self) -> anyhow::Result<async_nats::Subscriber> {
        Ok(self.client.subscribe("entheai.presence.ping").await?)
    }
```
(Requires `futures` in the worker — already present via env? add `futures = { workspace = true }` to `bin/entheai-worker/Cargo.toml` if the compile complains.)

- [ ] **Step 3: Build** — `cargo build -p entheai-federation -p entheai-worker` PASS; clippy clean. Commit:

```bash
git add crates/federation/src/lib.rs bin/entheai-worker/src/main.rs bin/entheai-worker/Cargo.toml Cargo.lock
git commit -m "feat(federation): worker presence heartbeat + count_workers (F2.2)"
```

---

## Task 3: `FederationExecutor` (impl the trait)

**Files:** `crates/federation/src/executor.rs`, `crates/federation/src/lib.rs`, `crates/federation/Cargo.toml`.

- [ ] **Step 1:** `crates/federation/Cargo.toml` — add `entheai-orchestrator = { path = "../orchestrator" }` (to impl the trait). **Check for a dependency cycle:** orchestrator must NOT depend on federation (it doesn't — the trait lives in orchestrator, federation depends on orchestrator). Confirm `cargo tree` has no cycle after adding.

- [ ] **Step 2:** Create `crates/federation/src/executor.rs`:

```rust
//! `CoderExecutor` impl: dispatch a coder to the fleet and squash-apply its
//! delta into the caller's worktree. Any miss returns `None` → local fallback.
use std::path::Path;
use std::sync::Arc;

use crate::{types, Federation, WorkItem};

pub struct FederationExecutor {
    fed: Federation,
    root: std::path::PathBuf,
}

impl FederationExecutor {
    pub fn new(fed: Federation, root: std::path::PathBuf) -> Arc<Self> {
        Arc::new(Self { fed, root })
    }
}

async fn git(dir: &Path, args: &[&str]) -> bool {
    tokio::process::Command::new("git").arg("-C").arg(dir).args(args)
        .output().await.map(|o| o.status.success()).unwrap_or(false)
}

#[async_trait::async_trait]
impl entheai_orchestrator::CoderExecutor for FederationExecutor {
    async fn workers_available(&self) -> bool {
        self.fed.count_workers(std::time::Duration::from_millis(800)).await > 0
    }

    async fn execute(&self, session: &str, index: usize, base_sha: &str, worktree_path: &Path, role: &str, task: &str) -> Option<String> {
        // 1. Bundle the base (from the repo root) + upload.
        let tmp = tempfile::tempdir().ok()?;
        let base_bundle = tmp.path().join("base.bundle");
        // Bundle the exact base_sha the worktree sits on.
        if !git(&self.root, &["bundle", "create", base_bundle.to_str()?, base_sha, "--branches=NONEXISTENT"]).await {
            // Fallback: bundle HEAD (== base_sha for a fresh fan-out).
            let _ = git(&self.root, &["bundle", "create", base_bundle.to_str()?, "HEAD"]).await;
        }
        let bkey = types::base_key(session, index);
        self.fed.put_bundle(&bkey, &tokio::fs::read(&base_bundle).await.ok()?).await.ok()?;

        // 2. Subscribe, dispatch, await.
        let mut sub = self.fed.subscribe_result(session, index).await.ok()?;
        self.fed.dispatch(&WorkItem {
            session: session.into(), index, role: role.into(), task: task.into(),
            base_bundle_key: bkey, base_sha: base_sha.into(),
        }).await.ok()?;
        let result = self.fed.await_result(&mut sub).await?;
        if !result.committed { return None; } // no-change/error → local fallback

        // 3. Squash-apply the delta bundle into the coder's worktree.
        let rb = tmp.path().join("result.bundle");
        tokio::fs::write(&rb, self.fed.get_bundle(&result.result_bundle_key).await.ok()?).await.ok()?;
        if !git(worktree_path, &["fetch", rb.to_str()?, "fed-work"]).await { return None; }
        if !git(worktree_path, &["merge", "--squash", "FETCH_HEAD"]).await { return None; }
        Some(result.log)
    }
}
```

Note: the `bundle create <sha> --branches=NONEXISTENT` trick bundles a single commit by ref; if it errors on the local git, the plan's fallback bundles HEAD. Verify Task 5 which path the machine takes and simplify to whichever works.

- [ ] **Step 3:** `lib.rs` — add `pub mod executor;` and `pub use executor::FederationExecutor;`. Add `tempfile` to `[dependencies]` (it's currently a dev-dep) — move it up.

- [ ] **Step 4: Build + clippy** — `cargo build -p entheai-federation`, clippy clean. Commit:

```bash
git add crates/federation/Cargo.toml crates/federation/src/lib.rs crates/federation/src/executor.rs Cargo.lock
git commit -m "feat(federation): FederationExecutor — dispatch + squash-apply into the worktree (F2.2)"
```

---

## Task 4: Bin wiring

**Files:** `bin/entheai/src/main.rs`, `bin/entheai/Cargo.toml`.

- [ ] **Step 1:** Add `entheai-federation = { path = "../../crates/federation" }` to `bin/entheai/Cargo.toml`.

- [ ] **Step 2:** In the one-shot fanout arm, build the executor when enabled and pass it:

```rust
                let fed_exec = if cfg.federation.enabled {
                    entheai_federation::Federation::connect(
                        &entheai_federation::FedOptions::from_config(&cfg.nats, &cfg.federation),
                    )
                    .await
                    .map(|f| entheai_federation::FederationExecutor::new(f, root.clone())
                        as std::sync::Arc<dyn entheai_orchestrator::CoderExecutor>)
                } else { None };
                let answer =
                    entheai_orchestrator::run_fanout(&cfg, &root, &prompt, events, pool, fed_exec).await?;
```

(Leave the F1 bus tee as-is; F2.2 adds the executor alongside it.)

- [ ] **Step 3: Build both feature configs** — `cargo build -p entheai` + `cargo build -p entheai --no-default-features` PASS; clippy clean. Commit:

```bash
git add bin/entheai/Cargo.toml bin/entheai/src/main.rs Cargo.lock
git commit -m "feat(bin): offload one-shot fan-out coders to the federation when enabled (F2.2)"
```

---

## Task 5: Live E2E + docs

- [ ] **Step 1: Workspace gate** — `cargo test --workspace` green; clippy clean; `cargo build -p entheai --no-default-features` PASS.

- [ ] **Step 2: Live E2E (this Mac + hub)** — throwaway `/tmp/fed.toml` (`[nats].enabled`, `[federation].enabled`, providers for a real coder OR use the worker's `--test-coder`). Since `run_fanout` calls the real orchestrator decompose (an LLM call) + the remote coder, verify with a real but tiny task:
  1. Start a worker: `entheai-worker --config /tmp/fed.toml --serve --test-coder 'printf "// touched by remote worker\n" >> README.md'` (bg).
  2. In a scratch git repo with a provider key in env, run: `entheai --config /tmp/fed.toml --fanout 'append a comment line to README.md'`.
  3. **Assert** the worker's log shows a claim, the fanout report shows the coder integrated, and the integration branch contains the remote change (`git log --all --oneline | head`; the appended line is present). Confirms decompose → remote coder → squash-apply → commit → integrate.
  4. Also verify **fallback**: kill the worker, re-run the same `--fanout` — it completes locally (a real local coder), proving the presence gate + fallback.

- [ ] **Step 3: Docs** — session doc + CHANGELOG Unreleased ("federation dispatch wired into `--fanout`"); note the TUI still runs local (executor threaded as `None` there — a follow-up). Commit.

---

## Self-Review

**Spec coverage (§4 remaining):** run_fanout offloads coders to the fleet ✅ T1/T3/T4; presence/heartbeat ✅ T2; leases/redelivery inherited from F2.1; local fallback (per-coder + presence-gated) ✅ T1/T3. **Still deferred (F2.3):** worker securefs/policy hardening (documented gate only here — a worker still runs model output with full tools; keep `--serve` to trusted nodes), TUI executor wiring, multi-worker load tests, shared-remote transport (option b).

**Risk control:** the run_fanout change is additive — `None` executor reproduces today's path exactly (proven by the unchanged test suite in T1 Step 7); the remote path only replaces *where the coder runs*, feeding the identical worktree state into the untouched commit/verify/integrate steps; any remote miss falls back to a local coder. `orchestrator` gains no NATS dependency (trait-only); the cycle check in T3 Step 1 guards the dep direction.

**Type consistency:** `CoderExecutor::{workers_available, execute}` (T1) is implemented by `FederationExecutor` (T3) and called in `run_coder_maybe_remote` (T1). `run_fanout`'s new `executor: Option<Arc<dyn CoderExecutor>>` matches both call sites (T1 Step 6 pass `None`; T4 passes `Some`). `FederationExecutor::new(fed, root)` (T3) matches the bin (T4).
