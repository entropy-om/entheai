# entheai-bus (F1 Event Bus) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish every fan-out run's lifecycle (`FanoutEvent`) to the live NATS hub on `entheai.fanout.<session>.*` subjects, so any tailnet subscriber sees runs live — while `orchestrator` stays completely NATS-agnostic and every run still works with NATS off/unreachable.

**Architecture:** A new `entheai-bus` crate wraps `async-nats`. It defines a serde `BusEvent` DTO (mirror of `orchestrator::FanoutEvent`, so orchestrator needs no serde-for-wire), a fail-safe `Bus::connect` (returns `None` on disabled/unreachable/auth-fail), and a `tee(bus, session, downstream)` that interposes a publisher between `run_fanout`'s existing `Option<UnboundedSender<FanoutEvent>>` and the UI. Config gains an opt-in `[nats]` block whose URL/token come from `.env` (never inlined in the tracked config). The bin (one-shot) and TUI (interactive) call `tee` right before `run_fanout`; with NATS off the tee is a zero-overhead identity and behavior is byte-for-byte unchanged.

**Tech Stack:** Rust (edition 2021, MSRV 1.80), `async-nats = "0.49"` (rustls-based — no system libs, preserves the headless/portable build), `serde`/`serde_json`, `tokio`, `uuid`. Mirrors the existing fail-safe background-service idiom of `crates/obsidian` (`start()` → `ObsidianSession` whose `Drop` aborts the task).

**Live substrate (already provisioned, verified):** hub `nats://entheai-nats.tail2870dc.ts.net:4222`, token auth (CONNECT `auth_token` field — `ConnectOptions::with_token` sets exactly this), tailnet-only. Creds are in the gitignored `.env` as `NATS_URL` + `NATS_TOKEN`. Design spec: `docs/superpowers/specs/2026-07-20-entheai-nats-federation-design.md` §3.

**Key seams confirmed in the codebase:**
- `crates/orchestrator/src/lib.rs:36` — `pub enum FanoutEvent { Fallback, Decomposed{tasks:Vec<(String,String)>}, CoderStarted{index:usize,role:String,task:String}, CoderFinished{index:usize,committed:bool,status:String}, Integrating{branches:usize}, Done{integration_branch:Option<String>,merged:usize,conflicted:usize} }` — `#[derive(Debug, Clone)]`, **no serde** (keep it that way).
- `crates/orchestrator/src/lib.rs:503` — `pub async fn run_fanout(config, root, task, events: Option<UnboundedSender<FanoutEvent>>, pool) -> anyhow::Result<String>` — **do not change this signature.**
- `bin/entheai/src/main.rs:118-122` — one-shot fanout call, passes `None` for events; `session_id` uuid already exists at line 102; `dotenvy::dotenv().ok()` at line 49.
- `crates/tui/src/lib.rs:468-476` — TUI fanout submit: creates `(ftx, frx)`, spawns `run_fanout(..., Some(ftx), pool)`. `config` is owned by `event_loop` and cloned into an `Arc` per submit.
- `crates/config/src/lib.rs:11-46` — `Config` struct; `ObsidianConfig` (lines ~/`pub struct ObsidianConfig`) is the sub-config template (env-name-string idiom).
- `crates/obsidian/src/lib.rs` — `pub fn start(...) -> ObsidianSession` + `impl Drop for ObsidianSession { fn drop … t.abort() }` is the exact fail-safe handle pattern to mirror.

**async-nats 0.49 API (verified via docs):**
- `async_nats::ConnectOptions::with_token(token: String).connect(url).await -> Result<Client, ConnectError>`.
- `ConnectOptions::connect` returns `Err` immediately if the server is unreachable (no `retry_on_initial_connect` set); `connection_timeout` defaults to 5s. So a dead hub → immediate `Err` → we return `None`. Fail-safe is automatic; no extra options needed.
- `Client::publish(subject: impl ToSubject, payload: Bytes).await -> Result<(), PublishError>` — `String` implements `ToSubject`; `Vec<u8>.into()` → `Bytes`.
- `async_nats::Client` is cheap to `Clone` (internally ref-counted).

---

## File Structure

- **Create `crates/bus/Cargo.toml`** — new crate `entheai-bus`; deps `async-nats`, `serde`, `serde_json`, `tokio`, `uuid`, `log`, `entheai-orchestrator` (for `FanoutEvent`), `entheai-config` (for `NatsConfig`).
- **Create `crates/bus/src/event.rs`** — `BusEvent` wire DTO, `From<&FanoutEvent>`, `subject_suffix()`. Pure/serde — fully unit-testable with no live server. Owns the wire format.
- **Create `crates/bus/src/lib.rs`** — `BusOptions` (+ `from_config`), `Bus` (`connect`, `publish_event`), `tee` + `BusSession`, `new_session_id`. Owns the NATS connection + the tee seam.
- **Modify `Cargo.toml`** (root) — add `crates/bus` to `members`; add `async-nats = "0.49"` to `[workspace.dependencies]`.
- **Modify `crates/config/src/lib.rs`** — add `NatsConfig` sub-config + `pub nats: NatsConfig` field on `Config` (+ tests).
- **Modify `bin/entheai/Cargo.toml`** — add `entheai-bus = { path = "../../crates/bus" }`.
- **Modify `bin/entheai/src/main.rs`** — connect + `tee` around the one-shot fanout call.
- **Modify `crates/tui/Cargo.toml`** — add `entheai-bus = { path = "../bus" }`.
- **Modify `crates/tui/src/lib.rs`** — connect the bus once in `event_loop`, `tee` per fanout submit.
- **Modify `entheai.toml`** (sample config, repo root — confirm path in Task 8) — documented `[nats]` block.

---

## Task 1: Scaffold the `entheai-bus` crate + workspace wiring

**Files:**
- Create: `crates/bus/Cargo.toml`
- Create: `crates/bus/src/lib.rs` (placeholder)
- Modify: `Cargo.toml` (root — `members` + `[workspace.dependencies]`)

- [ ] **Step 1: Add the crate to the workspace members**

In root `Cargo.toml`, add `"crates/bus"` to the `members` array (keep it alphabetically near the other crates, e.g. right after `"crates/obsidian"`):

```toml
members = ["crates/config", "crates/providers", "crates/core", "crates/tools", "crates/permission", "crates/tui", "crates/memory", "crates/radio", "crates/companion", "crates/router", "crates/orchestrator", "crates/mapper", "crates/mcp", "crates/skills", "crates/viz", "crates/launcher", "crates/obsidian", "crates/bus", "bin/entheai", "bin/entheai-worker", "bin/entheai-launch"]
```

- [ ] **Step 2: Add `async-nats` to workspace dependencies**

In root `Cargo.toml`, under `[workspace.dependencies]`, add (after the `rusqlite` line):

```toml
# NATS client for the federation event bus (F1). rustls-based (no system
# OpenSSL) so it preserves the portable/headless build.
async-nats = "0.49"
```

- [ ] **Step 3: Create `crates/bus/Cargo.toml`**

```toml
[package]
name = "entheai-bus"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
entheai-orchestrator = { path = "../orchestrator" }
entheai-config = { path = "../config" }
async-nats = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "sync"] }
uuid = { version = "1", features = ["v4"] }
log = "0.4"
```

- [ ] **Step 4: Create a placeholder `crates/bus/src/lib.rs`**

```rust
//! entheai-bus: the F1 federation event bus. Publishes the fan-out
//! orchestrator's `FanoutEvent` lifecycle to NATS (`entheai.fanout.<session>.*`)
//! so any tailnet subscriber can watch runs live. Fully fail-safe: with the
//! `[nats]` feature off or the hub unreachable, every entry point is a no-op and
//! the caller runs entirely locally.

mod event;
pub use event::BusEvent;
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build -p entheai-bus`
Expected: FAIL — `event` module does not exist yet (that's Task 3). This step just confirms the manifest + workspace wiring parse. If the error is anything other than the missing `event` module (e.g. a manifest/dependency resolution error), fix that before moving on.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/bus/Cargo.toml crates/bus/src/lib.rs
git commit -m "chore(bus): scaffold entheai-bus crate + async-nats workspace dep (F1)"
```

---

## Task 2: `NatsConfig` in `entheai-config`

**Files:**
- Modify: `crates/config/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of `crates/config/src/lib.rs`:

```rust
#[test]
fn nats_defaults_off_with_standard_env_names() {
    let cfg: Config = toml::from_str("").unwrap();
    assert!(!cfg.nats.enabled);
    assert_eq!(cfg.nats.url_env, "NATS_URL");
    assert_eq!(cfg.nats.token_env, "NATS_TOKEN");
}

#[test]
fn nats_block_parses_and_overrides() {
    let cfg: Config = toml::from_str(
        r#"
        [nats]
        enabled = true
        url_env = "MY_NATS_URL"
        token_env = "MY_NATS_TOKEN"
        "#,
    )
    .unwrap();
    assert!(cfg.nats.enabled);
    assert_eq!(cfg.nats.url_env, "MY_NATS_URL");
    assert_eq!(cfg.nats.token_env, "MY_NATS_TOKEN");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-config nats_ -- --nocolor`
Expected: FAIL to compile — `no field \`nats\` on type \`Config\``.

- [ ] **Step 3: Add the `NatsConfig` struct + `Config` field**

Add the `nats` field to the `Config` struct (after the `obsidian` field at `crates/config/src/lib.rs:45`):

```rust
    #[serde(default)]
    pub obsidian: ObsidianConfig,
    #[serde(default)]
    pub nats: NatsConfig,
}
```

Then add the struct + defaults near the other sub-configs (e.g. right after the `ObsidianConfig` block):

```rust
/// Federation event bus (`entheai-bus`, F1). Opt-in and fail-safe: with
/// `enabled = false` (the default) or an unreachable hub, entheai runs entirely
/// locally. The URL and token are read from the named environment variables
/// (populated from the gitignored `.env`), never inlined in the tracked config.
#[derive(Debug, Clone, Deserialize)]
pub struct NatsConfig {
    /// Master switch. When false, `Bus::connect` short-circuits to `None`.
    #[serde(default)]
    pub enabled: bool,
    /// Name of the env var holding the NATS URL (e.g. `nats://host:4222`).
    #[serde(default = "default_nats_url_env")]
    pub url_env: String,
    /// Name of the env var holding the NATS auth token.
    #[serde(default = "default_nats_token_env")]
    pub token_env: String,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url_env: default_nats_url_env(),
            token_env: default_nats_token_env(),
        }
    }
}

fn default_nats_url_env() -> String {
    "NATS_URL".to_string()
}

fn default_nats_token_env() -> String {
    "NATS_TOKEN".to_string()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p entheai-config nats_ -- --nocolor`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): add opt-in [nats] federation block (F1)"
```

---

## Task 3: `BusEvent` wire DTO + subject mapping

**Files:**
- Create: `crates/bus/src/event.rs`

- [ ] **Step 1: Write the failing test (create the file with impl stubs + tests)**

Create `crates/bus/src/event.rs`. Write the real types plus tests in one go (the DTO is small and the tests are the specification of the wire format):

```rust
//! Wire DTO for fan-out events. Mirrors `orchestrator::FanoutEvent` so the
//! `orchestrator` crate needs no serde-for-wire dependency, and owns the
//! subject-suffix + JSON contract that tailnet subscribers depend on.

use entheai_orchestrator::FanoutEvent;
use serde::Serialize;

/// JSON-serializable mirror of `FanoutEvent`, tagged by `event` kind. Published
/// to `entheai.fanout.<session>.<subject_suffix()>`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BusEvent {
    Fallback,
    Decomposed { tasks: Vec<(String, String)> },
    CoderStarted { index: usize, role: String, task: String },
    CoderFinished { index: usize, committed: bool, status: String },
    Integrating { branches: usize },
    Done { integration_branch: Option<String>, merged: usize, conflicted: usize },
}

impl BusEvent {
    /// Dotted subject suffix (under `entheai.fanout.<session>.`) — matches the
    /// taxonomy in the federation design spec §2.
    pub fn subject_suffix(&self) -> &'static str {
        match self {
            BusEvent::Fallback => "fallback",
            BusEvent::Decomposed { .. } => "decomposed",
            BusEvent::CoderStarted { .. } => "coder.started",
            BusEvent::CoderFinished { .. } => "coder.finished",
            BusEvent::Integrating { .. } => "integrating",
            BusEvent::Done { .. } => "done",
        }
    }
}

impl From<&FanoutEvent> for BusEvent {
    fn from(e: &FanoutEvent) -> Self {
        match e {
            FanoutEvent::Fallback => BusEvent::Fallback,
            FanoutEvent::Decomposed { tasks } => BusEvent::Decomposed { tasks: tasks.clone() },
            FanoutEvent::CoderStarted { index, role, task } => BusEvent::CoderStarted {
                index: *index,
                role: role.clone(),
                task: task.clone(),
            },
            FanoutEvent::CoderFinished { index, committed, status } => BusEvent::CoderFinished {
                index: *index,
                committed: *committed,
                status: status.clone(),
            },
            FanoutEvent::Integrating { branches } => BusEvent::Integrating { branches: *branches },
            FanoutEvent::Done { integration_branch, merged, conflicted } => BusEvent::Done {
                integration_branch: integration_branch.clone(),
                merged: *merged,
                conflicted: *conflicted,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_a_distinct_subject_suffix() {
        let suffixes = [
            BusEvent::Fallback.subject_suffix(),
            BusEvent::Decomposed { tasks: vec![] }.subject_suffix(),
            BusEvent::CoderStarted { index: 0, role: String::new(), task: String::new() }.subject_suffix(),
            BusEvent::CoderFinished { index: 0, committed: false, status: String::new() }.subject_suffix(),
            BusEvent::Integrating { branches: 0 }.subject_suffix(),
            BusEvent::Done { integration_branch: None, merged: 0, conflicted: 0 }.subject_suffix(),
        ];
        let unique: std::collections::HashSet<_> = suffixes.iter().collect();
        assert_eq!(unique.len(), suffixes.len(), "subject suffixes must be unique");
        assert_eq!(BusEvent::CoderStarted { index: 0, role: String::new(), task: String::new() }.subject_suffix(), "coder.started");
    }

    #[test]
    fn from_fanout_event_preserves_fields() {
        let fe = FanoutEvent::CoderFinished { index: 2, committed: true, status: "verified".into() };
        assert_eq!(
            BusEvent::from(&fe),
            BusEvent::CoderFinished { index: 2, committed: true, status: "verified".into() }
        );
    }

    #[test]
    fn serializes_to_tagged_json() {
        let json = serde_json::to_string(&BusEvent::Integrating { branches: 3 }).unwrap();
        assert_eq!(json, r#"{"event":"integrating","branches":3}"#);
    }

    #[test]
    fn done_serializes_all_fields() {
        let json = serde_json::to_string(&BusEvent::Done {
            integration_branch: Some("fanout/abc".into()),
            merged: 2,
            conflicted: 1,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"event":"done","integration_branch":"fanout/abc","merged":2,"conflicted":1}"#
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test -p entheai-bus event:: -- --nocolor`
Expected: PASS (4 tests). (`lib.rs` already declares `mod event;` from Task 1, so this compiles.)

- [ ] **Step 3: Commit**

```bash
git add crates/bus/src/event.rs
git commit -m "feat(bus): BusEvent wire DTO + subject mapping (F1)"
```

---

## Task 4: `Bus` connect + publish + `BusOptions::from_config`

**Files:**
- Modify: `crates/bus/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/bus/src/lib.rs` (below the `pub use`), a `#[cfg(test)]` module. These tests exercise the no-network paths (disabled → `None`; env resolution):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_returns_none_when_disabled() {
        let opts = BusOptions { enabled: false, url: Some("nats://127.0.0.1:4222".into()), token: None };
        assert!(Bus::connect(&opts).await.is_none());
    }

    #[tokio::test]
    async fn connect_returns_none_when_url_missing() {
        let opts = BusOptions { enabled: true, url: None, token: None };
        assert!(Bus::connect(&opts).await.is_none());
    }

    #[test]
    fn from_config_reads_named_env_vars() {
        // SAFETY: single-threaded test; unique var names avoid cross-test races.
        std::env::set_var("BUS_TEST_URL_F1", "nats://example:4222");
        std::env::set_var("BUS_TEST_TOKEN_F1", "s3cr3t");
        let cfg = entheai_config::NatsConfig {
            enabled: true,
            url_env: "BUS_TEST_URL_F1".into(),
            token_env: "BUS_TEST_TOKEN_F1".into(),
        };
        let opts = BusOptions::from_config(&cfg);
        assert!(opts.enabled);
        assert_eq!(opts.url.as_deref(), Some("nats://example:4222"));
        assert_eq!(opts.token.as_deref(), Some("s3cr3t"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-bus -- --nocolor`
Expected: FAIL to compile — `BusOptions`/`Bus` not defined.

- [ ] **Step 3: Implement `BusOptions` + `Bus`**

Add to `crates/bus/src/lib.rs` (after the `pub use event::BusEvent;` line):

```rust
use entheai_orchestrator::FanoutEvent;

/// Connection options resolved from the `[nats]` config + environment.
#[derive(Debug, Clone, Default)]
pub struct BusOptions {
    pub enabled: bool,
    pub url: Option<String>,
    pub token: Option<String>,
}

impl BusOptions {
    /// Resolve from the config block, reading the named env vars for URL/token.
    /// An unset or empty env var resolves to `None`, which makes `Bus::connect`
    /// a no-op (feature stays off) — the tracked config never inlines secrets.
    pub fn from_config(cfg: &entheai_config::NatsConfig) -> Self {
        let non_empty = |name: &str| std::env::var(name).ok().filter(|s| !s.is_empty());
        Self {
            enabled: cfg.enabled,
            url: non_empty(&cfg.url_env),
            token: non_empty(&cfg.token_env),
        }
    }
}

/// A connected NATS client for publishing fan-out events. Cheap to clone
/// (`async_nats::Client` is internally reference-counted).
#[derive(Clone)]
pub struct Bus {
    client: async_nats::Client,
}

impl Bus {
    /// Connect using the resolved options. Fail-safe: returns `None` when the
    /// feature is disabled, the URL is missing, or the connection/auth fails, so
    /// the caller runs entirely locally. `async_nats` returns an error
    /// immediately on an unreachable server (5s connection timeout, no initial
    /// retry), so a dead hub never stalls startup.
    pub async fn connect(opts: &BusOptions) -> Option<Bus> {
        if !opts.enabled {
            return None;
        }
        let Some(url) = opts.url.clone() else {
            log::warn!("nats: [nats].enabled but URL env is unset/empty — federation off");
            return None;
        };
        let connect = match &opts.token {
            Some(t) => async_nats::ConnectOptions::with_token(t.clone()),
            None => async_nats::ConnectOptions::new(),
        };
        match connect.connect(url.clone()).await {
            Ok(client) => {
                log::info!("nats: federation bus connected to {url}");
                Some(Bus { client })
            }
            Err(e) => {
                log::warn!("nats: connect to {url} failed ({e}) — federation off");
                None
            }
        }
    }

    /// Publish one fan-out event as JSON to `entheai.fanout.<session>.<suffix>`.
    /// Best-effort fire-and-forget (core NATS): any error is logged, never
    /// propagated (federation must never break a run).
    pub async fn publish_event(&self, session: &str, event: &FanoutEvent) {
        let dto = BusEvent::from(event);
        let subject = format!("entheai.fanout.{session}.{}", dto.subject_suffix());
        let payload = match serde_json::to_vec(&dto) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("nats: serialize event failed: {e}");
                return;
            }
        };
        if let Err(e) = self.client.publish(subject, payload.into()).await {
            log::warn!("nats: publish failed: {e}");
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p entheai-bus -- --nocolor`
Expected: PASS (event tests + 3 new tests). No live server is contacted (disabled/url-missing short-circuit before `connect`).

- [ ] **Step 5: Commit**

```bash
git add crates/bus/src/lib.rs
git commit -m "feat(bus): fail-safe Bus::connect + publish_event + BusOptions::from_config (F1)"
```

---

## Task 5: `tee` seam + `BusSession` handle + `new_session_id`

**Files:**
- Modify: `crates/bus/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/bus/src/lib.rs`:

```rust
    #[tokio::test]
    async fn tee_with_no_bus_is_identity_passthrough() {
        // With bus = None, tee returns the SAME downstream sender and an inert
        // handle — behavior is byte-for-byte unchanged from a NATS-less build.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<FanoutEvent>();
        let (returned, session) = tee(None, "sess".into(), Some(tx));
        let returned = returned.expect("downstream passed through");
        returned.send(FanoutEvent::Integrating { branches: 1 }).unwrap();
        match rx.recv().await {
            Some(FanoutEvent::Integrating { branches }) => assert_eq!(branches, 1),
            other => panic!("expected Integrating, got {other:?}"),
        }
        drop(session); // inert: no task to abort
    }

    #[tokio::test]
    async fn tee_with_no_bus_and_no_downstream_is_none() {
        let (returned, _session) = tee(None, "sess".into(), None);
        assert!(returned.is_none());
    }

    #[test]
    fn new_session_id_is_nonempty_and_hyphenless() {
        let id = new_session_id();
        assert!(!id.is_empty());
        assert!(!id.contains('-'), "simple uuid form has no hyphens");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-bus tee_ -- --nocolor`
Expected: FAIL to compile — `tee` / `new_session_id` not defined.

- [ ] **Step 3: Implement `tee`, `BusSession`, `new_session_id`**

Add to `crates/bus/src/lib.rs` (after the `impl Bus` block). Add `use tokio::sync::mpsc::UnboundedSender;` and `use tokio::task::JoinHandle;` to the top-of-file `use` section:

```rust
/// A running event-tee task. Dropping it aborts the tee (mirrors
/// `entheai_obsidian::ObsidianSession`), so a fan-out publisher never outlives
/// its run.
pub struct BusSession {
    task: Option<JoinHandle<()>>,
}

impl BusSession {
    fn inert() -> Self {
        Self { task: None }
    }
}

impl Drop for BusSession {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// Interpose a NATS publisher between `run_fanout` and its optional downstream
/// (UI) event sender. Returns the sender to hand to `run_fanout` plus a session
/// handle (drop = stop).
///
/// When `bus` is `None`, this is a zero-overhead identity: it returns
/// `downstream` unchanged and an inert handle, so behavior is exactly as a build
/// with NATS off. Otherwise it spawns a task that, for each event, forwards to
/// `downstream` FIRST (so UI latency stays independent of NATS) then publishes.
pub fn tee(
    bus: Option<Bus>,
    session: String,
    downstream: Option<UnboundedSender<FanoutEvent>>,
) -> (Option<UnboundedSender<FanoutEvent>>, BusSession) {
    let Some(bus) = bus else {
        return (downstream, BusSession::inert());
    };
    let (tee_tx, mut tee_rx) = tokio::sync::mpsc::unbounded_channel::<FanoutEvent>();
    let task = tokio::spawn(async move {
        while let Some(ev) = tee_rx.recv().await {
            if let Some(ds) = &downstream {
                let _ = ds.send(ev.clone());
            }
            bus.publish_event(&session, &ev).await;
        }
    });
    (Some(tee_tx), BusSession { task: Some(task) })
}

/// A fresh per-run session id for subject scoping (uuid v4, hyphen-free).
pub fn new_session_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p entheai-bus -- --nocolor`
Expected: PASS (all bus tests).

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p entheai-bus --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/bus/src/lib.rs
git commit -m "feat(bus): tee seam + BusSession (drop=stop) + new_session_id (F1)"
```

---

## Task 6: Wire the bus into the bin one-shot fanout path

**Files:**
- Modify: `bin/entheai/Cargo.toml`
- Modify: `bin/entheai/src/main.rs:118-122`

- [ ] **Step 1: Add the dependency**

In `bin/entheai/Cargo.toml`, add to `[dependencies]` (after the `entheai-obsidian` line):

```toml
entheai-bus = { path = "../../crates/bus" }
```

- [ ] **Step 2: Wire connect + tee around the one-shot fanout call**

In `bin/entheai/src/main.rs`, replace the fanout arm (currently lines 118-122):

```rust
            if cli.fanout {
                let pool = entheai_orchestrator::WorkerPool::new(cfg.router.max_parallel.max(1));
                let answer =
                    entheai_orchestrator::run_fanout(&cfg, &root, &prompt, None, pool).await?;
                println!("{answer}");
            } else {
```

with:

```rust
            if cli.fanout {
                let pool = entheai_orchestrator::WorkerPool::new(cfg.router.max_parallel.max(1));
                // Federation event bus (F1): opt-in + fail-safe. With `[nats]`
                // off or the hub unreachable, `connect` returns None and `tee`
                // hands `None` straight to run_fanout — behavior unchanged.
                let bus = entheai_bus::Bus::connect(
                    &entheai_bus::BusOptions::from_config(&cfg.nats),
                )
                .await;
                let (events, _bus_session) =
                    entheai_bus::tee(bus, session_id.clone(), None);
                let answer =
                    entheai_orchestrator::run_fanout(&cfg, &root, &prompt, events, pool).await?;
                println!("{answer}");
            } else {
```

(`session_id` is already defined at `bin/entheai/src/main.rs:102`. `_bus_session` is held to the end of the arm; its `Drop` aborts the tee after `run_fanout` returns.)

- [ ] **Step 3: Build**

Run: `cargo build -p entheai`
Expected: PASS.

- [ ] **Step 4: Verify the headless/portable build still has zero system libs**

Run: `cargo build -p entheai --no-default-features`
Expected: PASS (async-nats is rustls-based; no OpenSSL/GUI/audio libs pulled in). This guards the portable build from Task-1's regression surface.

- [ ] **Step 5: Verify NATS-off behavior is unchanged (no `.env`, default config)**

Run: `printf 'hello\n' | cargo run -q -p entheai -- --help >/dev/null 2>&1; echo "build ok"`
Expected: prints `build ok`. (A full fanout run needs provider keys; the real end-to-end publish check is Task 8 on dev-cx53. Here we only assert the wired binary builds and runs with NATS disabled by default — `cfg.nats.enabled` defaults to false, so `connect` returns `None`.)

- [ ] **Step 6: Commit**

```bash
git add bin/entheai/Cargo.toml bin/entheai/src/main.rs Cargo.lock
git commit -m "feat(bin): publish fan-out events to the NATS bus in one-shot mode (F1)"
```

---

## Task 7: Wire the bus into the TUI fanout submit path

**Files:**
- Modify: `crates/tui/Cargo.toml`
- Modify: `crates/tui/src/lib.rs` (`event_loop` body: connect once; `tee` at the fanout submit ~lines 460-476)

- [ ] **Step 1: Add the dependency**

In `crates/tui/Cargo.toml`, add to `[dependencies]` (after the `entheai-orchestrator` line):

```toml
entheai-bus = { path = "../bus" }
```

- [ ] **Step 2: Connect the bus once, near the top of `event_loop`**

`event_loop` is `async` and owns `config` before the loop. Immediately after `config` is available in `event_loop` (before the main loop starts — co-locate with where the fan-out `WorkerPool`/channels are set up, or just after the `let ... = config;` binding at the top of the function body), add:

```rust
    // Federation event bus (F1): connect once per TUI session, fail-safe. Cloned
    // into each fan-out submit's tee. `None` when `[nats]` is off/unreachable →
    // the tee is a pure identity and the UI event flow is unchanged.
    let bus = entheai_bus::Bus::connect(
        &entheai_bus::BusOptions::from_config(&config.nats),
    )
    .await;
```

Note: `config` here is the owned `entheai_config::Config` param (line 272 of `run`, threaded into `event_loop`). Access `config.nats` before `config` is moved/cloned into any `Arc`. If `event_loop` shadows/moves `config` early, read `config.nats` for `BusOptions::from_config` before that move (it only borrows).

- [ ] **Step 3: Tee at the fanout submit**

In the fanout branch of the submit handler (`crates/tui/src/lib.rs`, currently ~lines 468-476), replace:

```rust
                                let (ftx, frx) =
                                    mpsc::unbounded_channel::<entheai_orchestrator::FanoutEvent>();
                                fanout_rx = Some(frx);
                                tokio::spawn(async move {
                                    let res =
                                        entheai_orchestrator::run_fanout(&config, &root, &text, Some(ftx), pool)
                                            .await;
                                    let _ = result_tx.send(res.map_err(|e| e.to_string())).await;
                                });
```

with:

```rust
                                let (ftx, frx) =
                                    mpsc::unbounded_channel::<entheai_orchestrator::FanoutEvent>();
                                fanout_rx = Some(frx);
                                // Tee the UI event stream to the NATS bus (F1).
                                // Fresh per-run session id scopes the subjects;
                                // with the bus off this returns `Some(ftx)`
                                // unchanged. The BusSession is moved into the
                                // spawned task so it lives exactly as long as the
                                // run, then its Drop aborts the tee.
                                let (events, bus_session) = entheai_bus::tee(
                                    bus.clone(),
                                    entheai_bus::new_session_id(),
                                    Some(ftx),
                                );
                                tokio::spawn(async move {
                                    let _bus_session = bus_session;
                                    let res =
                                        entheai_orchestrator::run_fanout(&config, &root, &text, events, pool)
                                            .await;
                                    let _ = result_tx.send(res.map_err(|e| e.to_string())).await;
                                });
```

(`bus.clone()` — `Bus` is `Clone` and `Option<Bus>: Clone`. `bus` is captured by reference here; it lives in `event_loop`'s scope for the whole session. Moving `_bus_session` into the task ties the tee's lifetime to the run.)

- [ ] **Step 4: Build both feature configs**

Run: `cargo build -p entheai-tui`
Expected: PASS.

Run: `cargo build -p entheai-tui --no-default-features`
Expected: PASS (headless TUI still builds; async-nats adds no system libs).

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p entheai-tui --all-targets -- -D warnings`
Expected: no warnings. (If clippy flags the `let _bus_session = bus_session;` as `let_underscore`, that's intentional — it holds the guard; add `#[allow(clippy::let_underscore_untyped)]` only if clippy actually errors on it.)

- [ ] **Step 6: Commit**

```bash
git add crates/tui/Cargo.toml crates/tui/src/lib.rs Cargo.lock
git commit -m "feat(tui): tee fan-out events to the NATS bus per run (F1)"
```

---

## Task 8: Sample config docs + live end-to-end verification on dev-cx53

**Files:**
- Modify: `entheai.toml` (sample/default config — confirm exact path first)

- [ ] **Step 1: Locate the sample config file**

Run: `ls entheai.toml entheai.example.toml docs/**/entheai.toml 2>/dev/null; grep -rl "\[obsidian\]\|\[fanout\]" --include=*.toml . | grep -v target`
Expected: identifies the tracked sample config (the one already documenting `[obsidian]`/`[fanout]`). Use that path below as `<sample.toml>`. If there is no tracked sample config, skip the file edit and document `[nats]` in the federation spec's §6 instead (it's already there) — then go to Step 3.

- [ ] **Step 2: Add a documented (disabled) `[nats]` block**

Append to `<sample.toml>`:

```toml
# Federation event bus (entheai-bus, F1). Opt-in + fail-safe: with enabled =
# false (default) or an unreachable hub, entheai runs entirely locally. The URL
# and token are read from these env vars (populated from the gitignored .env) —
# never inline secrets in this tracked file.
[nats]
enabled = false
url_env = "NATS_URL"       # e.g. nats://entheai-nats.tail2870dc.ts.net:4222
token_env = "NATS_TOKEN"
```

Commit:

```bash
git add <sample.toml>
git commit -m "docs(config): sample [nats] federation block (F1)"
```

- [ ] **Step 3: Full workspace test + clippy**

Run: `cargo test --workspace`
Expected: PASS (baseline 281 + new bus/config tests).

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Live end-to-end verification on dev-cx53 (against the live hub)**

This is the spec §3 acceptance test. On the dev-cx53 sandbox (see the `dev-cx53-sandbox` memory), with `.env` carrying `NATS_URL` + `NATS_TOKEN`:

1. rsync the repo to the box (per the sandbox workflow) and build lean:
   `cargo build --bin entheai --no-default-features`
2. In one shell, subscribe on the box (or any tailnet host with the token):
   `nats sub 'entheai.fanout.>' --server "$NATS_URL" --token "$NATS_TOKEN"`
3. Create a throwaway config enabling nats (`[nats] enabled = true`), then run a small fanout task in a git repo, e.g.:
   `~/entheai-run.sh --fanout "create NATS_F1_PROOF.md with one line"`
4. **Assert** the subscriber prints the ordered event sequence:
   `entheai.fanout.<session>.decomposed` → `.coder.started` → `.coder.finished` → `.integrating` → `.done`
   (each a JSON body with the matching `"event"` tag).
5. **Assert fail-safe:** stop being able to reach the hub (temporarily set `NATS_URL` to a dead port) and re-run — the fanout still completes locally, with a single `nats: connect … failed … federation off` warning and no error.

- [ ] **Step 5: Record the result**

Update `docs/session/2026-07-20-e2e-polish.md` (or the current session doc) with the F1 outcome + the verified event sequence, and update `.remember/remember.md` to mark F1 done and F2 next. Commit:

```bash
git add docs/session/ .remember/ 2>/dev/null; git commit -m "docs(session): F1 event bus shipped + verified live on dev-cx53"
```

(`.remember/` is gitignored — the `git add` is best-effort; the session doc is the tracked record.)

---

## Self-Review

**Spec coverage (§3 of the design):**
- ✅ New `entheai-bus` crate wrapping `async-nats` — Task 1.
- ✅ `Bus::connect(cfg) -> Option<Bus>` (None on any failure) — Task 4.
- ✅ `Bus::publish_event(session, &FanoutEvent)` — Task 4.
- ✅ Wire DTO mirroring `FanoutEvent`, keeping `orchestrator` NATS-agnostic (DTO in `entheai-bus`) — Task 3.
- ✅ Seam = tee off the existing `Option<UnboundedSender<FanoutEvent>>`; orchestrator gains zero NATS knowledge (its signature is untouched) — Tasks 5-7.
- ✅ Subjects `entheai.fanout.<session>.*` matching §2 taxonomy (`decomposed`, `coder.started`, `coder.finished`, `integrating`, `done`; plus `fallback`) — Task 3.
- ✅ Opt-in + fail-safe (§1 principle 1): `[nats].enabled` default false; connect failure → local — Tasks 2, 4, 6, 7.
- ✅ Creds out of git via env-name indirection (§1 principle 5, §6 config) — Tasks 2, 4, 8.
- ✅ JSON on the wire (§1 principle 3) — Task 3.
- ✅ Verification (§3): subscribe from another host, assert ordered event kinds — Task 8.

**Placeholder scan:** none — every code step contains complete, compilable code; every command has an expected result. The only deliberately deferred lookup is the sample-config path (Task 8 Step 1 discovers it explicitly, with a documented fallback).

**Type consistency:** `BusEvent` (Task 3) is used identically in `publish_event` (Task 4). `BusOptions` fields (`enabled`/`url`/`token`, Task 4) match `from_config` (Task 4) and the call sites (Tasks 6-7). `tee(bus, session, downstream) -> (Option<UnboundedSender<FanoutEvent>>, BusSession)` (Task 5) matches both call sites exactly (Tasks 6-7). `new_session_id()` (Task 5) is used in Task 7. `NatsConfig { enabled, url_env, token_env }` (Task 2) matches `BusOptions::from_config(&cfg.nats)` (Task 4) and the sample config (Task 8). `run_fanout`'s signature is never modified — only the `events` argument value changes (`None`/`Some(ftx)` → `events`).

**Out of scope (correctly deferred to F2/F3):** JetStream work-queue, `entheai-worker` dispatch, git-bundle transport, KV shared state, subject-permission/nkey hardening. F1 is core-NATS fire-and-forget publish only.
