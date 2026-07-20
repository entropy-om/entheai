# Federation F2.1 — Distributed Coder over JetStream (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A coder sub-task can run on *another* tailnet node. A **dispatcher** bundles the repo, enqueues a `WorkItem` on a JetStream work-queue, and awaits a `WorkResult`; a **worker** (`entheai-worker --serve`) pulls the item, materializes the repo from a git bundle, runs the coder in an isolated dir, bundles the delta back through the object store, and publishes the result. The dispatcher applies the delta to a fresh branch.

**Architecture:** New crate `entheai-federation` wraps async-nats **JetStream**: a WorkQueue stream (`ENTHEAI_WORK`, subject `entheai.work.coder`) for durable, exactly-one-worker delivery + leases/redelivery; a JetStream **object store** (`entheai-bundles`) for git bundles; **core NATS** pub/sub for the synchronous result (`entheai.result.<session>.<index>`). `entheai-worker` gains `--serve` (consume) and `--dispatch` (enqueue+collect) modes, reusing the existing `run_coder_once`. The `[federation]` config gates it; creds come from the existing `[nats]`/`.env`. **`run_fanout` is NOT modified in this slice** — dispatch is a standalone path so the transport can be proven before rewiring the orchestrator (F2.2).

**Tech Stack:** Rust (2021, MSRV 1.80). `async-nats` (JetStream + object store, workspace, rustls), `serde`/`serde_json`, `tokio`, `anyhow`, `tokio-util`/`futures` for `AsyncRead`, `log`. Git via `tokio::process::Command` (mirrors `crates/orchestrator/src/worktree.rs`). Hub: the live `entheai-nats` (JetStream enabled, verified).

**Verified async-nats JetStream API (do not deviate):**
- `let js = async_nats::jetstream::new(client);`
- Stream: `js.get_or_create_stream(jetstream::stream::Config { name, subjects: vec!["entheai.work.>".into()], retention: jetstream::stream::RetentionPolicy::WorkQueue, ..Default::default() }).await?`
- Publish (with ack): `js.publish(subject, payload.into()).await?.await?`
- Pull consumer: `stream.get_or_create_consumer("coder-workers", jetstream::consumer::pull::Config { durable_name: Some("coder-workers".into()), filter_subject: "entheai.work.coder".into(), ack_policy: AckPolicy::Explicit, ack_wait: Duration, max_deliver: 3, ..Default::default() }).await?`
- Consume: `let mut msgs = consumer.messages().await?;` then `while let Some(m) = msgs.next().await { let m = m?; /* m.payload */ m.ack().await?; }` (needs `futures::StreamExt`).
- Object store: `js.create_object_store(jetstream::object_store::Config { bucket: "entheai-bundles".into(), ..Default::default() }).await?` / `js.get_object_store("entheai-bundles").await?`. Put: `store.put("key", &mut &bytes[..]).await?` (2nd arg impls `tokio::io::AsyncRead`; `&[u8]` does). Get: `let mut obj = store.get("key").await?; let mut buf = Vec::new(); obj.read_to_end(&mut buf).await?;` (needs `tokio::io::AsyncReadExt`). Object names allow `[-/_=.a-zA-Z0-9]`, no leading/trailing dot.
- `async_nats::Client` (and the JetStream `Context`) are cheap-clone.

**Seams (verified):**
- `entheai_orchestrator::run_coder_once(config, role, task, worktree_path) -> String` (never Errs; error captured as `"error: coder failed: …"`).
- `entheai_orchestrator::worktree::resolve_base(root, "HEAD") -> String`, `is_git_repo(root) -> bool`.
- Git subprocess idiom: `tokio::process::Command::new("git").arg("-C").arg(dir).args([...])`.
- `bin/entheai-worker` today: one-shot `--role/--task/--worktree` → `run_coder_once`. Keep that mode; add `--serve`/`--dispatch`.
- Config `Config.nats: NatsConfig { enabled, url_env, token_env }` (from F1). `entheai_bus::BusOptions::from_config(&cfg.nats)` resolves url/token from env — reuse it.

---

## File Structure

- **Create `crates/federation/src/types.rs`** — `WorkItem`, `WorkResult` serde DTOs + object-key helpers. Pure; fully unit-tested.
- **Create `crates/federation/src/repo.rs`** — git-bundle transport: `bundle_base`, `materialize_from_bundle`, `bundle_delta`, `apply_delta_bundle`. Pure git subprocess; unit-tested against temp repos (the testable meat).
- **Create `crates/federation/src/lib.rs`** — `FedOptions`, `Federation` (JetStream wrapper: `connect`, `ensure_infra`, `dispatch`, `claim`/`Claimed::ack`, `put_bundle`/`get_bundle`, `publish_result`, `await_result`). Live-verified.
- **Create `crates/federation/Cargo.toml`**.
- **Modify `Cargo.toml`** — add `crates/federation` member.
- **Modify `crates/config/src/lib.rs`** — `[federation]` block (`FederationConfig`).
- **Modify `bin/entheai-worker/Cargo.toml` + `src/main.rs`** — `--serve` / `--dispatch` modes.

---

## Task 1: Scaffold `entheai-federation` + config

**Files:** Create `crates/federation/Cargo.toml`, `crates/federation/src/lib.rs` (placeholder); Modify root `Cargo.toml`, `crates/config/src/lib.rs`.

- [ ] **Step 1: Add member** — in root `Cargo.toml` `members`, add `"crates/federation"` (after `"crates/bus"`).

- [ ] **Step 2: `crates/federation/Cargo.toml`**

```toml
[package]
name = "entheai-federation"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
entheai-config = { path = "../config" }
entheai-bus = { path = "../bus" }
async-nats = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "process", "io-util", "fs"] }
futures = { workspace = true }
anyhow = { workspace = true }
log = "0.4"

[dev-dependencies]
tempfile = "3"
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "process", "io-util", "fs"] }
```

- [ ] **Step 3: `crates/federation/src/lib.rs` placeholder**

```rust
//! entheai-federation (F2): dispatch coder sub-tasks to worker nodes over NATS
//! JetStream. A WorkQueue stream delivers each `WorkItem` to exactly one worker;
//! git bundles travel through the JetStream object store; results return over
//! core NATS. Fail-safe: any NATS failure leaves the caller to run locally.

pub mod repo;
pub mod types;
```

- [ ] **Step 4: `[federation]` config** — in `crates/config/src/lib.rs`, add the field to `Config` (after `nats`):

```rust
    #[serde(default)]
    pub federation: FederationConfig,
```

and the struct + defaults near `NatsConfig`:

```rust
/// Distributed swarm (F2). Opt-in; reuses `[nats]` for the connection. `role`
/// selects whether this process dispatches work, serves as a worker, or both.
#[derive(Debug, Clone, Deserialize)]
pub struct FederationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_fed_role")]
    pub role: String, // "auto" | "worker" | "dispatch"
    #[serde(default = "default_fed_deadline_secs")]
    pub deadline_secs: u64,
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self { enabled: false, role: default_fed_role(), deadline_secs: default_fed_deadline_secs() }
    }
}

fn default_fed_role() -> String { "auto".to_string() }
fn default_fed_deadline_secs() -> u64 { 600 }
```

- [ ] **Step 5: Verify** — `cargo build -p entheai-config` PASS; `cargo build -p entheai-federation` FAILS only on the missing `repo`/`types` modules (Tasks 2–3). Add a config test:

```rust
#[test]
fn federation_defaults_off() {
    let cfg: Config = toml::from_str("").unwrap();
    assert!(!cfg.federation.enabled);
    assert_eq!(cfg.federation.role, "auto");
    assert_eq!(cfg.federation.deadline_secs, 600);
}
```
Run: `cargo test -p entheai-config federation_defaults_off` → PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/federation/Cargo.toml crates/federation/src/lib.rs crates/config/src/lib.rs
git commit -m "chore(federation): scaffold entheai-federation crate + [federation] config (F2)"
```

---

## Task 2: `WorkItem` / `WorkResult` DTOs + key helpers

**Files:** Create `crates/federation/src/types.rs`.

- [ ] **Step 1: Failing tests** — create `types.rs`:

```rust
//! Wire DTOs + object-store key helpers for the F2 work-queue.
use serde::{Deserialize, Serialize};

/// A unit of coder work enqueued on `entheai.work.coder`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkItem {
    pub session: String,
    pub index: usize,
    pub role: String,
    pub task: String,
    /// Object-store key of the base repo bundle the worker must materialize.
    pub base_bundle_key: String,
    /// The commit the bundle checks out to (worker branches from here).
    pub base_sha: String,
}

/// A worker's outcome, published to `entheai.result.<session>.<index>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkResult {
    pub session: String,
    pub index: usize,
    /// "committed" | "no-change" | "error".
    pub status: String,
    pub committed: bool,
    /// Object-store key of the delta bundle (empty when nothing changed).
    pub result_bundle_key: String,
    /// The coder's captured output/log (truncated).
    pub log: String,
}

/// Core-NATS subject a worker publishes its result on / the dispatcher awaits.
pub fn result_subject(session: &str, index: usize) -> String {
    format!("entheai.result.{session}.{index}")
}
/// Object-store key for a session's base bundle.
pub fn base_key(session: &str, index: usize) -> String {
    format!("base/{session}/{index}")
}
/// Object-store key for a session/index's result delta bundle.
pub fn result_key(session: &str, index: usize) -> String {
    format!("result/{session}/{index}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_and_keys_are_stable() {
        assert_eq!(result_subject("abc", 2), "entheai.result.abc.2");
        assert_eq!(base_key("abc", 0), "base/abc/0");
        assert_eq!(result_key("abc", 1), "result/abc/1");
    }

    #[test]
    fn work_item_json_round_trips() {
        let w = WorkItem { session: "s".into(), index: 1, role: "coder".into(), task: "t".into(), base_bundle_key: base_key("s", 1), base_sha: "deadbeef".into() };
        let j = serde_json::to_vec(&w).unwrap();
        assert_eq!(serde_json::from_slice::<WorkItem>(&j).unwrap(), w);
    }

    #[test]
    fn work_result_json_round_trips() {
        let r = WorkResult { session: "s".into(), index: 1, status: "committed".into(), committed: true, result_bundle_key: result_key("s", 1), log: "ok".into() };
        let j = serde_json::to_vec(&r).unwrap();
        assert_eq!(serde_json::from_slice::<WorkResult>(&j).unwrap(), r);
    }
}
```

- [ ] **Step 2: Run** — `cargo test -p entheai-federation types:: -- --color=never` → PASS (lib.rs already declares `pub mod types;`).

- [ ] **Step 3: Commit**

```bash
git add crates/federation/src/types.rs
git commit -m "feat(federation): WorkItem/WorkResult DTOs + object-store key helpers (F2)"
```

---

## Task 3: Git-bundle transport (`repo.rs`) — the testable core

**Files:** Create `crates/federation/src/repo.rs`.

- [ ] **Step 1: Failing test** — create `repo.rs` with a full round-trip test (dispatcher bundles → worker materializes + changes + delta-bundles → dispatcher applies):

```rust
//! Git-bundle transport for F2: move a repo (and a coder's delta) between the
//! dispatcher and a worker as self-contained bundles. All git runs via
//! `tokio::process::Command` (mirrors the orchestrator's worktree helpers).
use std::path::Path;

async fn git(dir: &Path, args: &[&str]) -> anyhow::Result<(bool, String)> {
    let out = tokio::process::Command::new("git").arg("-C").arg(dir).args(args).output().await
        .map_err(|e| anyhow::anyhow!("spawn git -C {} {:?}: {e}", dir.display(), args))?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}
async fn git_ok(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = tokio::process::Command::new("git").arg("-C").arg(dir).args(args).output().await?;
    if !out.status.success() {
        anyhow::bail!("git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Create a base bundle of `repo`'s HEAD under `out` (a `.bundle` path). Bundles
/// the branch `fed-base` pointing at HEAD so a clone lands on a named branch.
pub async fn bundle_base(repo: &Path, out: &Path) -> anyhow::Result<String> {
    let base_sha = git_ok(repo, &["rev-parse", "HEAD"]).await?.trim().to_string();
    // A fresh branch ref for the bundle (force in case it exists).
    git_ok(repo, &["branch", "-f", "fed-base", &base_sha]).await?;
    let out_s = out.to_string_lossy();
    git_ok(repo, &["bundle", "create", &out_s, "fed-base"]).await?;
    Ok(base_sha)
}

/// Clone a base bundle into `dest` and check out `fed-base`, then create a
/// working branch `fed-work`. Returns the worktree path (`dest`).
pub async fn materialize_from_bundle(bundle: &Path, dest: &Path) -> anyhow::Result<()> {
    let bundle_s = bundle.to_string_lossy();
    let dest_s = dest.to_string_lossy();
    let out = tokio::process::Command::new("git")
        .args(["clone", "-b", "fed-base", &bundle_s, &dest_s]).output().await?;
    if !out.status.success() {
        anyhow::bail!("git clone bundle failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    git_ok(dest, &["checkout", "-b", "fed-work"]).await?;
    // Identity so commits succeed in the ephemeral clone.
    git_ok(dest, &["config", "user.email", "worker@entheai"]).await?;
    git_ok(dest, &["config", "user.name", "entheai-worker"]).await?;
    Ok(())
}

/// After a coder changed files in `worktree`: stage+commit; if nothing changed
/// return Ok(None); else bundle `base_sha..fed-work` to `out` and return the
/// new sha.
pub async fn commit_and_bundle_delta(worktree: &Path, base_sha: &str, msg: &str, out: &Path) -> anyhow::Result<Option<String>> {
    git_ok(worktree, &["add", "-A"]).await?;
    let (clean, _) = git(worktree, &["diff", "--cached", "--quiet"]).await?;
    if clean { return Ok(None); } // nothing staged
    git_ok(worktree, &["commit", "-m", msg]).await?;
    let new_sha = git_ok(worktree, &["rev-parse", "HEAD"]).await?.trim().to_string();
    let out_s = out.to_string_lossy();
    let range = format!("{base_sha}..fed-work");
    git_ok(worktree, &["bundle", "create", &out_s, &range]).await?;
    Ok(Some(new_sha))
}

/// In the dispatcher's `repo` (which has `base_sha`), fetch the worker's delta
/// bundle into a fresh local branch `branch`. Returns the fetched tip sha.
pub async fn apply_delta_bundle(repo: &Path, bundle: &Path, branch: &str) -> anyhow::Result<String> {
    let bundle_s = bundle.to_string_lossy();
    let refspec = format!("fed-work:refs/heads/{branch}");
    git_ok(repo, &["fetch", &bundle_s, &refspec]).await?;
    Ok(git_ok(repo, &["rev-parse", branch]).await?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn init_repo(dir: &Path) {
        git_ok(dir, &["init", "-q"]).await.unwrap();
        git_ok(dir, &["config", "user.email", "t@t"]).await.unwrap();
        git_ok(dir, &["config", "user.name", "t"]).await.unwrap();
        tokio::fs::write(dir.join("README.md"), "base\n").await.unwrap();
        git_ok(dir, &["add", "-A"]).await.unwrap();
        git_ok(dir, &["commit", "-q", "-m", "base"]).await.unwrap();
    }

    #[tokio::test]
    async fn full_bundle_round_trip_applies_the_delta() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = tmp.path().join("disp");
        tokio::fs::create_dir_all(&dispatcher).await.unwrap();
        init_repo(&dispatcher).await;

        // Dispatcher bundles base.
        let base_bundle = tmp.path().join("base.bundle");
        let base_sha = bundle_base(&dispatcher, &base_bundle).await.unwrap();

        // Worker materializes, changes a file, delta-bundles.
        let work = tmp.path().join("work");
        materialize_from_bundle(&base_bundle, &work).await.unwrap();
        assert_eq!(tokio::fs::read_to_string(work.join("README.md")).await.unwrap(), "base\n");
        tokio::fs::write(work.join("NEW.md"), "from worker\n").await.unwrap();
        let result_bundle = tmp.path().join("result.bundle");
        let new_sha = super::commit_and_bundle_delta(&work, &base_sha, "worker change", &result_bundle).await.unwrap();
        assert!(new_sha.is_some());

        // Dispatcher applies the delta to a branch.
        let tip = apply_delta_bundle(&dispatcher, &result_bundle, "fed/test").await.unwrap();
        assert_eq!(tip, new_sha.unwrap());
        // The new file exists on that branch.
        let show = git_ok(&dispatcher, &["show", "fed/test:NEW.md"]).await.unwrap();
        assert_eq!(show, "from worker\n");
    }

    #[tokio::test]
    async fn no_change_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let disp = tmp.path().join("disp");
        tokio::fs::create_dir_all(&disp).await.unwrap();
        init_repo(&disp).await;
        let base_bundle = tmp.path().join("b.bundle");
        let base_sha = bundle_base(&disp, &base_bundle).await.unwrap();
        let work = tmp.path().join("w");
        materialize_from_bundle(&base_bundle, &work).await.unwrap();
        let rb = tmp.path().join("r.bundle");
        assert!(commit_and_bundle_delta(&work, &base_sha, "noop", &rb).await.unwrap().is_none());
    }
}
```

- [ ] **Step 2: Run** — `cargo test -p entheai-federation repo:: -- --color=never` → PASS (2 tests). If `git clone -b fed-base <bundle>` errors on the runner's git version, adjust to `git clone <bundle> <dest>` then `git -C dest checkout fed-base` and re-run; keep the assertions.

- [ ] **Step 3: Clippy + commit**

```bash
cargo clippy -p entheai-federation --all-targets -- -D warnings
git add crates/federation/src/repo.rs
git commit -m "feat(federation): git-bundle transport (base + delta round-trip), unit-proven"
```

---

## Task 4: `Federation` JetStream wrapper

**Files:** Modify `crates/federation/src/lib.rs`.

- [ ] **Step 1: Implement** (no unit test — JetStream needs a live server; a live integration test comes in Task 7). Add to `lib.rs`:

```rust
use std::time::Duration;
use futures::StreamExt;
use tokio::io::AsyncReadExt;

pub use types::{WorkItem, WorkResult};

const WORK_STREAM: &str = "ENTHEAI_WORK";
const WORK_SUBJECT: &str = "entheai.work.coder";
const BUNDLES_BUCKET: &str = "entheai-bundles";
const DURABLE: &str = "coder-workers";

/// Resolved federation options (reuses the `[nats]` connection).
#[derive(Debug, Clone, Default)]
pub struct FedOptions {
    pub enabled: bool,
    pub url: Option<String>,
    pub token: Option<String>,
    pub deadline: Duration,
}

impl FedOptions {
    pub fn from_config(nats: &entheai_config::NatsConfig, fed: &entheai_config::FederationConfig) -> Self {
        let bus = entheai_bus::BusOptions::from_config(nats);
        Self { enabled: fed.enabled, url: bus.url, token: bus.token, deadline: Duration::from_secs(fed.deadline_secs) }
    }
}

#[derive(Clone)]
pub struct Federation {
    js: async_nats::jetstream::Context,
    client: async_nats::Client,
    deadline: Duration,
}

/// A claimed work item with its ack handle.
pub struct Claimed {
    pub item: WorkItem,
    msg: async_nats::jetstream::Message,
}
impl Claimed {
    pub async fn ack(&self) { let _ = self.msg.ack().await; }
}

impl Federation {
    /// Connect + ensure infra. Fail-safe: `None` on disabled/unreachable/error.
    pub async fn connect(opts: &FedOptions) -> Option<Federation> {
        if !opts.enabled { return None; }
        let url = opts.url.clone()?;
        let connect = match &opts.token {
            Some(t) => async_nats::ConnectOptions::with_token(t.clone()),
            None => async_nats::ConnectOptions::new(),
        };
        let client = match connect.connect(url.clone()).await {
            Ok(c) => c,
            Err(e) => { log::warn!("federation: connect {url} failed ({e}) — off"); return None; }
        };
        let js = async_nats::jetstream::new(client.clone());
        let fed = Federation { js, client, deadline: opts.deadline };
        if let Err(e) = fed.ensure_infra().await {
            log::warn!("federation: ensure_infra failed ({e}) — off");
            return None;
        }
        Some(fed)
    }

    async fn ensure_infra(&self) -> anyhow::Result<()> {
        self.js.get_or_create_stream(async_nats::jetstream::stream::Config {
            name: WORK_STREAM.into(),
            subjects: vec!["entheai.work.>".into()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            ..Default::default()
        }).await?;
        self.js.create_object_store(async_nats::jetstream::object_store::Config {
            bucket: BUNDLES_BUCKET.into(), ..Default::default()
        }).await.ok(); // create is idempotent-ish; ignore already-exists
        Ok(())
    }

    pub async fn put_bundle(&self, key: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let store = self.js.get_object_store(BUNDLES_BUCKET).await?;
        let mut src = bytes;
        store.put(key, &mut src).await?;
        Ok(())
    }

    pub async fn get_bundle(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let store = self.js.get_object_store(BUNDLES_BUCKET).await?;
        let mut obj = store.get(key).await?;
        let mut buf = Vec::new();
        obj.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    pub async fn dispatch(&self, item: &WorkItem) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(item)?;
        self.js.publish(WORK_SUBJECT.to_string(), payload.into()).await?.await?;
        Ok(())
    }

    /// Block for the next work item (bounded by `expires`). Returns None on timeout.
    pub async fn claim(&self, expires: Duration) -> anyhow::Result<Option<Claimed>> {
        let stream = self.js.get_stream(WORK_STREAM).await?;
        let consumer = stream.get_or_create_consumer(DURABLE, async_nats::jetstream::consumer::pull::Config {
            durable_name: Some(DURABLE.into()),
            filter_subject: WORK_SUBJECT.into(),
            ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
            ack_wait: self.deadline,
            max_deliver: 3,
            ..Default::default()
        }).await?;
        let mut batch = consumer.batch().max_messages(1).expires(expires).messages().await?;
        match batch.next().await {
            Some(Ok(msg)) => {
                let item: WorkItem = serde_json::from_slice(&msg.payload)?;
                Ok(Some(Claimed { item, msg }))
            }
            _ => Ok(None),
        }
    }

    pub async fn publish_result(&self, r: &WorkResult) -> anyhow::Result<()> {
        let subject = types::result_subject(&r.session, r.index);
        self.client.publish(subject, serde_json::to_vec(r)?.into()).await?;
        self.client.flush().await?;
        Ok(())
    }

    /// Subscribe first, THEN return a handle; the dispatcher must subscribe
    /// before dispatching so the core-NATS result isn't missed.
    pub async fn subscribe_result(&self, session: &str, index: usize) -> anyhow::Result<async_nats::Subscriber> {
        Ok(self.client.subscribe(types::result_subject(session, index)).await?)
    }

    /// Await one result on an existing subscription, bounded by `self.deadline`.
    pub async fn await_result(&self, sub: &mut async_nats::Subscriber) -> Option<WorkResult> {
        match tokio::time::timeout(self.deadline, sub.next()).await {
            Ok(Some(msg)) => serde_json::from_slice(&msg.payload).ok(),
            _ => None,
        }
    }
}
```

- [ ] **Step 2: Build** — `cargo build -p entheai-federation` PASS. `cargo clippy -p entheai-federation --all-targets -- -D warnings` clean. (If `consumer.batch().max_messages(1)` naming differs in 0.49, use `.batch().max_messages(1)` per the pull `Batch` builder; adjust to the exact builder method and note it.)

- [ ] **Step 3: Commit**

```bash
git add crates/federation/src/lib.rs
git commit -m "feat(federation): JetStream Federation wrapper — dispatch/claim/bundle/result (F2)"
```

---

## Task 5: `entheai-worker --serve`

**Files:** Modify `bin/entheai-worker/Cargo.toml`, `bin/entheai-worker/src/main.rs`.

- [ ] **Step 1: Deps** — add to `bin/entheai-worker/Cargo.toml`:

```toml
entheai-federation = { path = "../../crates/federation" }
entheai-bus = { path = "../../crates/bus" }
tempfile = "3"
log = "0.4"
env_logger = "0.11"
```
and ensure `tokio` has `["macros","rt-multi-thread","process","io-util","fs","time"]`.

- [ ] **Step 2: Add modes to the CLI** — restructure `Cli` to support a mode. Add fields (keep the one-shot fields optional):

```rust
    /// Run as a worker: pull WorkItems from the federation queue and process them.
    #[arg(long)]
    serve: bool,
    /// For testing: replace the LLM coder with a shell command run in the worktree.
    #[arg(long)]
    test_coder: Option<String>,
```

- [ ] **Step 3: Implement serve loop** — add a function and dispatch on `cli.serve` in `main` before the one-shot path:

```rust
async fn run_serve(config: &Config, test_coder: Option<&str>) -> anyhow::Result<()> {
    let opts = entheai_federation::FedOptions::from_config(&config.nats, &config.federation);
    let fed = entheai_federation::Federation::connect(&opts).await
        .ok_or_else(|| anyhow::anyhow!("federation not available (check [federation].enabled + [nats] creds)"))?;
    log::info!("entheai-worker: serving the coder work-queue");
    loop {
        let Some(claimed) = fed.claim(std::time::Duration::from_secs(20)).await? else { continue };
        let item = claimed.item.clone();
        log::info!("claimed work {}::{} role={}", item.session, item.index, item.role);
        let result = process_one(&fed, config, &item, test_coder).await
            .unwrap_or_else(|e| entheai_federation::WorkResult {
                session: item.session.clone(), index: item.index, status: "error".into(),
                committed: false, result_bundle_key: String::new(), log: format!("error: {e}"),
            });
        fed.publish_result(&result).await.ok();
        claimed.ack().await;
    }
}

async fn process_one(fed: &entheai_federation::Federation, config: &Config, item: &entheai_federation::WorkItem, test_coder: Option<&str>) -> anyhow::Result<entheai_federation::WorkResult> {
    let tmp = tempfile::tempdir()?;
    let base_bundle = tmp.path().join("base.bundle");
    tokio::fs::write(&base_bundle, fed.get_bundle(&item.base_bundle_key).await?).await?;
    let work = tmp.path().join("work");
    entheai_federation::repo::materialize_from_bundle(&base_bundle, &work).await?;

    // Coder step: real LLM by default; a shell command in test mode.
    let log = match test_coder {
        Some(cmd) => {
            let out = tokio::process::Command::new("sh").arg("-c").arg(cmd).current_dir(&work).output().await?;
            format!("test-coder rc={}: {}", out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout))
        }
        None => entheai_orchestrator::run_coder_once(config, &item.role, &item.task, &work).await,
    };

    let result_bundle = tmp.path().join("result.bundle");
    match entheai_federation::repo::commit_and_bundle_delta(&work, &item.base_sha, &format!("fed: {}", item.task), &result_bundle).await? {
        Some(_new_sha) => {
            let key = entheai_federation::types::result_key(&item.session, item.index);
            fed.put_bundle(&key, &tokio::fs::read(&result_bundle).await?).await?;
            Ok(entheai_federation::WorkResult { session: item.session.clone(), index: item.index, status: "committed".into(), committed: true, result_bundle_key: key, log: truncate(&log) })
        }
        None => Ok(entheai_federation::WorkResult { session: item.session.clone(), index: item.index, status: "no-change".into(), committed: false, result_bundle_key: String::new(), log: truncate(&log) }),
    }
}

fn truncate(s: &str) -> String { s.chars().take(2000).collect() }
```

In `main`, before the existing one-shot block:

```rust
    if cli.serve {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
        return run_serve(&config, cli.test_coder.as_deref()).await;
    }
```

Make the one-shot `role`/`task`/`worktree` args `Option<...>` and guard with a clear error when `--serve`/`--dispatch` isn't set and they're missing.

- [ ] **Step 4: Build** — `cargo build -p entheai-worker` PASS. `cargo clippy -p entheai-worker --all-targets -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add bin/entheai-worker/Cargo.toml bin/entheai-worker/src/main.rs Cargo.lock
git commit -m "feat(worker): --serve mode — pull work, materialize, run coder, bundle result (F2)"
```

---

## Task 6: `entheai-worker --dispatch`

**Files:** Modify `bin/entheai-worker/src/main.rs`.

- [ ] **Step 1: Add flags**

```rust
    /// Dispatch a single coder task to the federation queue and apply the result.
    #[arg(long)]
    dispatch: bool,
```
(Reuse `--role`/`--task`; run against the current dir as the repo.)

- [ ] **Step 2: Implement**

```rust
async fn run_dispatch(config: &Config, role: &str, task: &str) -> anyhow::Result<()> {
    let opts = entheai_federation::FedOptions::from_config(&config.nats, &config.federation);
    let fed = entheai_federation::Federation::connect(&opts).await
        .ok_or_else(|| anyhow::anyhow!("federation not available"))?;
    let repo = std::env::current_dir()?;
    let session = uuid_like();
    let index = 0usize;

    // Bundle the repo base, upload it.
    let tmp = tempfile::tempdir()?;
    let base_bundle = tmp.path().join("base.bundle");
    let base_sha = entheai_federation::repo::bundle_base(&repo, &base_bundle).await?;
    let base_key = entheai_federation::types::base_key(&session, index);
    fed.put_bundle(&base_key, &tokio::fs::read(&base_bundle).await?).await?;

    // Subscribe BEFORE dispatch so the result isn't missed.
    let mut sub = fed.subscribe_result(&session, index).await?;
    fed.dispatch(&entheai_federation::WorkItem {
        session: session.clone(), index, role: role.into(), task: task.into(),
        base_bundle_key: base_key, base_sha: base_sha.clone(),
    }).await?;
    println!("dispatched {session}::{index} — awaiting a worker…");

    match fed.await_result(&mut sub).await {
        Some(r) if r.committed => {
            let rb = tmp.path().join("result.bundle");
            tokio::fs::write(&rb, fed.get_bundle(&r.result_bundle_key).await?).await?;
            let branch = format!("fed/{session}-{index}");
            let tip = entheai_federation::repo::apply_delta_bundle(&repo, &rb, &branch).await?;
            println!("worker committed → branch {branch} @ {tip}");
        }
        Some(r) => println!("worker returned status={} (no change applied)\n{}", r.status, r.log),
        None => println!("no worker result within the deadline — dispatch fell through (run locally)."),
    }
    Ok(())
}

fn uuid_like() -> String {
    // Avoid a uuid dep here: derive from the base bundle path + pid is enough
    // for a per-run subject; keep it [a-z0-9].
    format!("d{}", std::process::id())
}
```
Dispatch in `main` after the `--serve` check:
```rust
    if cli.dispatch {
        let role = cli.role.clone().unwrap_or_else(|| "coder".into());
        let task = cli.task.clone().ok_or_else(|| anyhow::anyhow!("--dispatch needs --task"))?;
        return run_dispatch(&config, &role, &task).await;
    }
```

- [ ] **Step 3: Build + clippy** — both clean.

- [ ] **Step 4: Commit**

```bash
git add bin/entheai-worker/src/main.rs
git commit -m "feat(worker): --dispatch mode — enqueue a coder task + apply the result (F2)"
```

---

## Task 7: Live end-to-end verification (this Mac + the live hub)

**Files:** none (verification) — then docs.

- [ ] **Step 1: Workspace gate** — `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo build -p entheai --no-default-features` PASS (federation stays out of the default bin; only entheai-worker links it).

- [ ] **Step 2: Enable federation in a throwaway config** — copy `entheai.toml` → `/tmp/fed.toml`, add:
```toml
[nats]
enabled = true
[federation]
enabled = true
deadline_secs = 120
```
(`.env` already has `NATS_URL`/`NATS_TOKEN`.)

- [ ] **Step 3: Transport E2E (zero LLM cost)** — in a scratch git repo:
  1. `git init` a temp repo with one commit.
  2. Terminal A (worker, deterministic coder): `entheai-worker --config /tmp/fed.toml --serve --test-coder 'printf "from the worker\n" > FED_PROOF.md'` (run in background).
  3. Terminal B (dispatcher): from the temp repo, `entheai-worker --config /tmp/fed.toml --dispatch --task 'create FED_PROOF.md'`.
  4. **Assert:** the dispatcher prints `worker committed → branch fed/... @ <sha>`, and `git show fed/<...>:FED_PROOF.md` == `from the worker`. This proves stream + object-store bundle round-trip + consumer + result over the live hub.
  5. Kill the worker; clean the JetStream test objects if desired (`nats` not installed — leave them; they're WorkQueue/limits-bounded).

- [ ] **Step 4: (Optional) real-coder smoke** — same but drop `--test-coder`, task `create FED_PROOF.md with one line`, with provider keys in `.env`. Confirms `run_coder_once` runs remotely. (Costs a small LLM call.)

- [ ] **Step 5: Docs** — add a `## Federation (distributed swarm)` note to the session doc + a `[federation]` block to `entheai.toml` (disabled) + a `--serve`/`--dispatch` line to the README's More commands. Commit.

---

## Self-Review

**Spec coverage (§4):** WorkItem→work-queue (WorkQueue retention, exactly-one delivery) ✅ T2/T4; worker pull-consumer materialize→coder→result ✅ T5; git-bundle transport over object store (recommended option a) ✅ T3/T4; WorkResult collect + apply to a branch ✅ T6; leases/redelivery via `ack_wait`+`max_deliver` ✅ T4; local fallback on no-result ✅ T6. **Deferred to F2.2 (documented):** run_fanout integration (dispatch replaces local coders), presence/heartbeat, multi-index batches, securefs worker hardening, shared-remote transport (option b). This slice proves the loop end-to-end without touching the orchestrator hot path.

**Placeholder scan:** none — every step has complete code; Task 4/Task 2 note the two exact API spots (`batch()` builder name; clone-`-b` fallback) to adjust against async-nats 0.49 if they differ, with the fix inline.

**Type consistency:** `WorkItem`/`WorkResult` (T2) are produced/consumed identically in `Federation` (T4), worker `process_one` (T5), and dispatcher (T6). `repo::{bundle_base,materialize_from_bundle,commit_and_bundle_delta,apply_delta_bundle}` (T3) signatures match their call sites (T5/T6). `FedOptions::from_config(&cfg.nats, &cfg.federation)` (T4) matches the config (T1) and both worker modes.

**Security note:** the worker runs model-generated code with full tools in an ephemeral clone on whatever node serves — same yolo posture as local fan-out, now off-machine. F2.1 is opt-in (`[federation].enabled`), tailnet-only (WireGuard), and the test path uses a deterministic shell coder. Real remote coders should run only on nodes you trust until the securefs/policy hardening (F2.2) lands — call this out in the docs.
