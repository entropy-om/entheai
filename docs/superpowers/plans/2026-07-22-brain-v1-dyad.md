# BRAIN v1 — the dyad memory system — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `entheai --fanout` (agy/AgyExecutor) for subagent-driven execution, task-by-task, per standing preference. Verify every fanout result against a real `cargo test`/`cargo clippy` run before trusting its self-report — this session already caught two fanout runs that reported success incorrectly (a hallucinated crate, and a "clippy clean" claim that wasn't). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `docs/superpowers/specs/2026-07-22-brain-v1-dyad-design.md`. Slice 1 closes two real wiring gaps (interactive-session memory ingest never fires; frozen nodes are built but never called). Slice 2 adds `BrainJudge`, a background local-LLM worker that proactively wakes relevant frozen nodes from ambient activity, not just on-request retrieval.

**Architecture:** Slice 1 reuses `crates/tui`'s existing `AgentEvent` channel (do not remove it — the adk-rust migration that would have deleted it is paused) to carry a new `FrozenWoke` variant back to the TUI's `BrainState`. `PromptProcessor` gains an owned `FrozenStore` and two passthrough methods; `run_task_with_memory` (unchanged signature) calls them alongside its existing retrieval block. Slice 2's `BrainJudge` lives in `crates/memory-pp` (no UI dependency), is spawned and owned by `crates/tui` (mirrors how the TUI already owns `event_tx`/`event_rx` for `AgentEvent`), and talks to `crates/viz::BrainState` only through its own dedicated channel — same pattern, new instance.

**Tech Stack:** Rust, existing entheai crates only. No new dependencies for Slice 1. Slice 2 reuses `entheai_providers::Provider` (already supports osaurus) — no adk-rust, no new HTTP client.

## Global Constraints

- **Never modify `crates/tui/src/lib.rs`'s uncommitted `/config` menu code** (the `Status::ConfigMenu` variant and its `handle_key`/`status_line` arms) — that's the user's own in-progress work, unrelated to this plan. If a diff conflicts with it, resolve around it, never overwrite it.
- Every task's line-number references are **approximate as of plan-writing time** — re-`grep` the actual current line before editing, per this session's established discipline (the file has shifted twice already during this session from unrelated concurrent edits).
- Every new/modified crate must pass `cargo test -p <crate>` and `cargo clippy -p <crate> --all-targets -- -D warnings` before its commit step. Run clippy for real — do not trust a subagent's "clippy clean" claim without re-running it yourself (this session hit that exact false claim twice on the adk-rust plan).
- The adk-rust migration (`docs/superpowers/plans/2026-07-22-adk-rust-core-migration.md`) is paused, not abandoned — do not delete `crates/providers`, `AgentEvent`, or anything else that plan's later tasks depend on.

---

## Slice 1 — close the wiring gaps

### Task 1: PromptProcessor gains a FrozenStore

**Files:**
- Modify: `crates/memory-pp/src/processor.rs`
- Modify: `crates/memory-pp/src/lib.rs` (re-export, if `FrozenStore`'s constructor changes)

**Interfaces:**
- Consumes: `entheai_memory_pp::frozen::{FrozenStore, FrozenNode, activate}` (already exported at crate root per `crates/memory-pp/src/lib.rs:17`).
- Produces: `PromptProcessor::wake_frozen(&self, prompt: &str, top_k: usize) -> Vec<FrozenNode>`, `PromptProcessor::activate_frozen(&self, node: &FrozenNode, deadline: Duration) -> String` — consumed by Task 2.

- [ ] **Step 1: Write the failing test**

Add to `crates/memory-pp/src/processor.rs`'s existing `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn wake_frozen_matches_trigger_and_activates() {
        use crate::frozen::{FrozenNode, FrozenStore};
        let node = FrozenNode {
            name: "nixos".into(),
            domain: "cloud".into(),
            triggers: vec!["hetzner".into()],
            mcp: None,
            rank: 1.0,
            knowledge: "use nix flakes".into(),
        };
        let frozen = FrozenStore::from_nodes(vec![node]);
        let raw = RawStore::open_memory().unwrap();
        let pp = PromptProcessor::new(
            raw, Box::new(StubMesh), Box::new(StubMarqant),
            Duration::from_millis(50), 16, 1 << 20, frozen,
        );
        let woken = pp.wake_frozen("deploy to hetzner please", 1);
        assert_eq!(woken.len(), 1);
        assert_eq!(woken[0].name, "nixos");
        let brief = pp.activate_frozen(&woken[0], Duration::from_millis(50)).await;
        assert!(brief.contains("frozen:nixos"));
    }

    #[test]
    fn wake_frozen_no_match_returns_empty() {
        use crate::frozen::FrozenStore;
        let raw = RawStore::open_memory().unwrap();
        let pp = PromptProcessor::new(
            raw, Box::new(StubMesh), Box::new(StubMarqant),
            Duration::from_millis(50), 16, 1 << 20, FrozenStore::from_nodes(vec![]),
        );
        assert!(pp.wake_frozen("anything", 1).is_empty());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-memory-pp wake_frozen -- --nocapture`
Expected: FAIL — `PromptProcessor::new` doesn't take a 7th `frozen` argument yet, and `wake_frozen`/`activate_frozen` don't exist.

- [ ] **Step 3: Add the field and methods**

In `crates/memory-pp/src/processor.rs`, add to the `PromptProcessor` struct:

```rust
    frozen: crate::frozen::FrozenStore,
```

Update `PromptProcessor::new`'s signature and body to accept and store it as the last parameter:

```rust
    pub fn new(
        raw: RawStore,
        mesh: Box<dyn MeshSearch>,
        marqant: Box<dyn Marqant>,
        deadline: Duration,
        recall_k: usize,
        max_ingest_bytes: usize,
        frozen: crate::frozen::FrozenStore,
    ) -> Self {
        Self { raw, mesh, marqant, deadline, recall_k, max_ingest_bytes, frozen }
    }
```

Add the two new methods (near `raw()`):

```rust
    /// Reactive frozen-node match against the current prompt (deterministic
    /// trigger + lexical relevance — see `frozen::FrozenStore::wake`).
    pub fn wake_frozen(&self, prompt: &str, top_k: usize) -> Vec<crate::frozen::FrozenNode> {
        self.frozen.wake(prompt, top_k)
    }

    /// Distil a woken node's knowledge through this processor's own compressor.
    pub async fn activate_frozen(&self, node: &crate::frozen::FrozenNode, deadline: Duration) -> String {
        crate::frozen::activate(node, self.marqant.as_ref(), self.max_ingest_bytes, deadline).await
    }
```

- [ ] **Step 4: Fix every other `PromptProcessor::new` call site**

```bash
grep -rn "PromptProcessor::new(" crates bin
```
Each existing call site (in `processor.rs`'s other tests, and `bin/entheai/src/main.rs`'s `build_prompt_processor`) needs a 7th argument. For `bin/entheai/src/main.rs`'s real construction, load the real corpus: `entheai_memory_pp::frozen::FrozenStore::load(&std::path::Path::new("frozen"))` (relative to the run cwd, matching the existing `frozen/` directory at the repo root — the same 11-node corpus `crates/memory-pp/src/frozen.rs`'s own `loads_real_frozen_dir_nodes` test already reads). For every other test call site, pass `crate::frozen::FrozenStore::from_nodes(vec![])` (empty — those tests don't exercise frozen behavior, an empty store just means `wake_frozen` always returns nothing, which is the correct neutral default).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p entheai-memory-pp -- --nocapture`
Expected: all pass, including the 2 new tests.

- [ ] **Step 6: Clippy**

Run: `cargo clippy -p entheai-memory-pp --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Build the dependent crates**

Run: `cargo build -p entheai-core -p entheai 2>&1 | tail -60`
Expected: fails at every `PromptProcessor::new` call site outside `memory-pp` too (`crates/core`'s tests, if any construct one directly) — fix each the same way as Step 4, using an empty `FrozenStore::from_nodes(vec![])` for test call sites.

- [ ] **Step 8: Commit**

```bash
git add crates/memory-pp/src/processor.rs bin/entheai/src/main.rs
git commit -m "feat(memory-pp): PromptProcessor owns a FrozenStore, wake_frozen/activate_frozen"
```

---

### Task 2: Wire frozen-wake into run_task_with_memory + a new FrozenWoke event

**Files:**
- Modify: `crates/core/src/lib.rs`

**Interfaces:**
- Consumes: `PromptProcessor::wake_frozen`/`activate_frozen` (Task 1).
- Produces: `AgentEvent::FrozenWoke { name: String }` — a new variant, consumed by Task 3 (TUI rendering).

- [ ] **Step 1: Write the failing test**

Add to `crates/core/src/lib.rs`'s test module (near the other `run_task_with_memory` tests — re-`grep` `run_task_with_memory` in the test module to find them, since exact line numbers shift):

```rust
    #[tokio::test]
    async fn run_task_with_memory_wakes_frozen_node_and_emits_event() {
        use entheai_memory_pp::frozen::{FrozenNode, FrozenStore};
        let node = FrozenNode {
            name: "nixos".into(), domain: "cloud".into(),
            triggers: vec!["hetzner".into()], mcp: None, rank: 1.0,
            knowledge: "use nix flakes".into(),
        };
        let raw = entheai_memory_pp::RawStore::open_memory().unwrap();
        let pp = entheai_memory_pp::PromptProcessor::new(
            raw, Box::new(entheai_memory_pp::StubMesh), Box::new(entheai_memory_pp::StubMarqant),
            std::time::Duration::from_millis(50), 16, 1 << 20,
            FrozenStore::from_nodes(vec![node]),
        );
        // ... build a stub Provider + Agent + registry + policy the same way this
        // file's other run_task_with_memory tests already do (copy that
        // fixture setup verbatim — do not re-derive it) ...
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let messages = vec![entheai_providers::ChatMessage::user("please deploy to hetzner")];
        let _ = agent.run_task_with_memory(
            messages, &registry, &policy, &mut prompter, Some(tx),
            None, Some(&pp), scope(),
        ).await;
        let mut saw_wake = false;
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::FrozenWoke { name } = ev {
                assert_eq!(name, "nixos");
                saw_wake = true;
            }
        }
        assert!(saw_wake, "expected a FrozenWoke event for the matched node");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-core run_task_with_memory_wakes_frozen -- --nocapture`
Expected: FAIL — `AgentEvent::FrozenWoke` doesn't exist, and nothing calls `wake_frozen` yet.

- [ ] **Step 3: Add the event variant**

In `crates/core/src/lib.rs`'s `AgentEvent` enum (near `Token(String)`):

```rust
    /// A frozen node's knowledge is being surfaced (reactively, matched
    /// against the current message, or proactively — see BRAIN v1 Slice 2).
    FrozenWoke { name: String },
```

- [ ] **Step 4: Wire the wake into the retrieval block**

In `run_task_with_memory`, inside the `if let Some(mem) = memory` block's retrieval logic (the block that currently only handles `pp`/`mem.retrieve_before`), add frozen-wake alongside it — actually **run frozen-wake unconditionally on `pp`, independent of whether `memory` is `Some`** (frozen nodes are a `PromptProcessor` concern, not a `MemoryRuntime` one — re-read the current function body first via a fresh read, since this plan's earlier research of this function predates any of Slice 1's changes, then place this block appropriately relative to the existing `if let Some(user_idx) = ...` structure rather than nested only inside the `memory`-gated branch):

```rust
        if let Some(p) = pp {
            if let Some(user_idx) = messages.iter().rposition(|m| m.role == "user") {
                let user_msg = messages[user_idx].content.clone();
                let woken = p.wake_frozen(&user_msg, 1);
                for node in &woken {
                    let brief = p.activate_frozen(node, self.stream_deadline()).await; // see Step 4a
                    messages.insert(user_idx, entheai_providers::ChatMessage::system(brief));
                    if let Some(tx) = &events {
                        let _ = tx.send(AgentEvent::FrozenWoke { name: node.name.clone() });
                    }
                }
            }
        }
```

- [ ] **Step 4a: Resolve the deadline value**

`activate_frozen` needs a `Duration` deadline. Check whether `Agent<P>` already has a configured deadline/timeout field to reuse (it doesn't, per the struct fields read during BRAIN v1's design research: `provider`, `model`, `max_turns` only) — use a fixed local constant instead, matching the existing pattern of small fixed timeouts elsewhere in this codebase (e.g. `crates/memory-pp`'s own `search_deadline_ms` default of 1500ms): add `const FROZEN_ACTIVATE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);` near the top of `crates/core/src/lib.rs` and use that instead of `self.stream_deadline()` (which doesn't exist — remove that placeholder call).

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p entheai-core run_task_with_memory_wakes_frozen -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Run the full crate's existing tests**

Run: `cargo test -p entheai-core -- --nocapture`
Expected: all pass — confirm no existing `run_task_with_memory` test's fixture (built with an empty `FrozenStore`, per Task 1 Step 4) regresses now that the wake logic runs unconditionally when `pp` is present.

- [ ] **Step 7: Clippy**

Run: `cargo clippy -p entheai-core --all-targets -- -D warnings`

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/lib.rs
git commit -m "feat(core): wire FrozenStore::wake into run_task_with_memory, emit FrozenWoke"
```

---

### Task 3: Thread memory + pp + scope into the TUI, render FrozenWoke

**Files:**
- Modify: `crates/tui/src/lib.rs`
- Modify: `crates/tui/Cargo.toml`
- Modify: `bin/entheai/src/main.rs`

**Interfaces:**
- Consumes: `AgentEvent::FrozenWoke` (Task 2), `entheai_memory::{MemoryRuntime, MemoryScope}`, `entheai_memory_pp::PromptProcessor` (all pre-existing), `entheai_viz::BrainState::wake_frozen` (pre-existing, `crates/viz/src/brain.rs:85`).

- [ ] **Step 1: Add the entheai-memory dependency**

In `crates/tui/Cargo.toml`, add (matching the sibling-path style already used for `entheai-core`):

```toml
entheai-memory = { path = "../memory" }
```

(`entheai-memory-pp` is very likely already a dependency, since `AgentEvent`/`Agent<P>` come through `entheai-core` which already depends on it — confirm with `grep entheai-memory-pp crates/tui/Cargo.toml`; add it explicitly if missing.)

- [ ] **Step 2: Extend `run` and `event_loop` signatures**

Re-`grep` `pub async fn run<P: Provider` and `async fn event_loop<P: Provider` in `crates/tui/src/lib.rs` for current line numbers. Add three parameters after `companion_tx` to both (they mirror each other 1:1 per the existing comment above `event_loop`):

```rust
    memory: Option<std::sync::Arc<entheai_memory::MemoryRuntime>>,
    pp: Option<std::sync::Arc<entheai_memory_pp::PromptProcessor>>,
    scope: entheai_memory::MemoryScope,
```

Thread all three through `run`'s call into `event_loop` (add as the last three arguments in that call).

- [ ] **Step 3: Swap the agent call at the interactive-run spawn site**

Re-`grep` `.run_task(history, &registry, &policy, &mut prompter, Some(event_tx))` for its current line (was 757 as of this plan's writing). Immediately before the `tokio::spawn(async move { ... })` block containing it, clone what needs to move in (matching the existing `let agent = Arc::clone(&agent);` style right above it):

```rust
                                let mem = memory.clone();
                                let pp_clone = pp.clone();
                                let sc = entheai_memory::MemoryScope {
                                    task_id: format!("turn-{}", uuid::Uuid::new_v4()),
                                    ..scope.clone()
                                };
```

Replace the call inside the spawned block:

```rust
                                    let res = agent
                                        .run_task_with_memory(
                                            history, &registry, &policy, &mut prompter,
                                            Some(event_tx), mem.as_deref(), pp_clone.as_deref(), sc,
                                        )
                                        .await;
```

Check whether `uuid` is already a dependency of `crates/tui` (`grep uuid crates/tui/Cargo.toml`); add `uuid = { workspace = true, features = ["v4"] }` if not (the workspace already has `uuid = { version = "1", features = ["v4"] }` per `crates/orchestrator/Cargo.toml` — confirm the workspace dep exists at the root `Cargo.toml` and reuse it, don't redeclare a different version).

- [ ] **Step 4: Handle the new AgentEvent variant**

Find the TUI's `AgentEvent::` match arms (`grep -n "AgentEvent::" crates/tui/src/lib.rs`) — there's one match block handling `Thinking`/`Token`/`ToolStarted`/`ToolFinished` as they arrive off `events_rx`. Add a new arm:

```rust
                        AgentEvent::FrozenWoke { name } => {
                            app.brain.wake_frozen(&name);
                        }
```

- [ ] **Step 5: Update bin/entheai's interactive call site**

In `bin/entheai/src/main.rs`, the `None =>` arm (currently ~line 319-333) calls `entheai_tui::run(...)`. Build the same `runtime`/`pp`/`scope` the oneshot branch already builds (mirror lines ~285-303 exactly, reusing `shared_memory` which is already in scope from line 219), then pass them:

```rust
        None => {
            let companion_tx = companion.as_ref().map(|c| c.state_tx.clone());
            let runtime = shared_memory.clone().map(|m| {
                std::sync::Arc::new(entheai_memory::MemoryRuntime::new(m, memory_runtime_config(&cfg.memory)))
            });
            let pp = build_prompt_processor(&cfg)?.map(std::sync::Arc::new);
            if let Some(p) = &pp {
                let retention = cfg.memory.prompt_processing.as_ref()
                    .map(|c| c.raw_retention_days).unwrap_or(90);
                p.prune(retention).await;
            }
            let scope = entheai_memory::MemoryScope {
                session_id: session_id.clone(),
                task_id: "tui".to_string(),
                cwd: root.clone(),
                role: None,
            };
            entheai_tui::run(
                agent, registry, policy, model_id.clone(), cfg, root.clone(),
                cli.fanout, system_prompt, companion_tx, runtime, pp, scope,
            )
            .await?;
        }
```

- [ ] **Step 6: Build and test**

```bash
cargo build -p entheai-tui -p entheai 2>&1 | tail -80
cargo test -p entheai-tui -- --nocapture
```
Expected: builds clean, existing TUI tests still pass.

- [ ] **Step 7: Clippy**

```bash
cargo clippy -p entheai-tui -p entheai --all-targets -- -D warnings
```

- [ ] **Step 8: Manual verification (the actual gap-closure proof)**

Start a real interactive session (`entheai` with no subcommand) against a configured provider, send one message containing a frozen-node trigger word (e.g. "hetzner" or "nixos" — check `frozen/nixos.md`'s `triggers` list for the exact words), and confirm: (a) the brain ring visibly glows for that node, (b) after the session, `sqlite3 <raw_path> "select count(*) from ..."` (check `raw_store.rs`'s schema for the actual table name) shows at least one new row that wasn't there before the message — proving Slice 1's core claim (interactive sessions now write to the raw store) is real, not just compiling.

- [ ] **Step 9: Commit**

```bash
git add crates/tui/src/lib.rs crates/tui/Cargo.toml bin/entheai/src/main.rs Cargo.lock
git commit -m "feat(tui): thread memory+pp into the interactive loop, render FrozenWoke"
```

---

## Slice 2 — BrainJudge: proactive surfacing

### Task 4: BrainJudge core — recent-activity buffer + relevance judgment

**Files:**
- Create: `crates/memory-pp/src/judge.rs`
- Modify: `crates/memory-pp/src/lib.rs` (add `pub mod judge; pub use judge::{BrainJudge, BrainJudgeEvent};`)

**Interfaces:**
- Consumes: `entheai_providers::{Provider, ChatMessage}` (already a dependency, `crates/memory-pp/src/processor.rs` already imports `ChatMessage`; add `Provider` if not already imported), `entheai_memory_pp::frozen::FrozenStore` (Task 1).
- Produces: `pub struct BrainJudge`, `pub enum BrainJudgeEvent { Woke(String) }`, `BrainJudge::new(provider: Arc<dyn Provider>, model: String, frozen: FrozenStore, cooldown: Duration) -> (Self, mpsc::UnboundedReceiver<BrainJudgeEvent>)`, `BrainJudge::notify(&self, activity: &str)` — consumed by Task 5.

- [ ] **Step 1: Write the failing test**

```rust
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use entheai_providers::{ChatMessage, Provider, ProviderError, StreamEvent};

use crate::frozen::FrozenStore;

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProvider {
        reply: String,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        async fn stream_chat(&self, _model: &str, _messages: &[ChatMessage])
            -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError> {
            unimplemented!("BrainJudge only uses complete()")
        }
        async fn complete(&self, _model: &str, _messages: &[ChatMessage], _schemas: &[serde_json::Value])
            -> Result<entheai_providers::AssistantResponse, ProviderError> {
            Ok(entheai_providers::AssistantResponse { content: self.reply.clone(), tool_calls: vec![] })
        }
        async fn stream_complete(&self, _model: &str, _messages: &[ChatMessage], _schemas: &[serde_json::Value], _tx: Option<futures::channel::mpsc::UnboundedSender<String>>)
            -> Result<entheai_providers::AssistantResponse, ProviderError> {
            unimplemented!("BrainJudge only uses complete()")
        }
    }

    fn node(name: &str) -> crate::frozen::FrozenNode {
        crate::frozen::FrozenNode {
            name: name.into(), domain: "".into(), triggers: vec![], mcp: None,
            rank: 1.0, knowledge: format!("knowledge about {name}"),
        }
    }

    #[tokio::test]
    async fn judge_wakes_the_node_the_model_names() {
        let frozen = FrozenStore::from_nodes(vec![node("nixos"), node("ngrok")]);
        let provider = Arc::new(FakeProvider { reply: "nixos".to_string() });
        let (judge, mut events) = BrainJudge::new(provider, "test/model".into(), frozen, Duration::from_millis(10));
        judge.notify("editing flake.nix").await;
        let ev = tokio::time::timeout(Duration::from_secs(1), events.recv()).await
            .expect("event arrives").expect("channel open");
        assert!(matches!(ev, BrainJudgeEvent::Woke(name) if name == "nixos"));
    }

    #[tokio::test]
    async fn judge_reply_of_none_wakes_nothing() {
        let frozen = FrozenStore::from_nodes(vec![node("nixos")]);
        let provider = Arc::new(FakeProvider { reply: "none".to_string() });
        let (judge, mut events) = BrainJudge::new(provider, "test/model".into(), frozen, Duration::from_millis(10));
        judge.notify("unrelated activity").await;
        let result = tokio::time::timeout(Duration::from_millis(200), events.recv()).await;
        assert!(result.is_err(), "no event should arrive for a 'none' judgment");
    }

    #[tokio::test]
    async fn judge_cooldown_suppresses_rapid_repeat_triggers() {
        let frozen = FrozenStore::from_nodes(vec![node("nixos")]);
        let provider = Arc::new(FakeProvider { reply: "nixos".to_string() });
        let (judge, mut events) = BrainJudge::new(provider, "test/model".into(), frozen, Duration::from_secs(60));
        judge.notify("first").await;
        let _first = tokio::time::timeout(Duration::from_secs(1), events.recv()).await.unwrap().unwrap();
        judge.notify("second, within cooldown").await;
        let result = tokio::time::timeout(Duration::from_millis(200), events.recv()).await;
        assert!(result.is_err(), "second trigger inside the cooldown window must be suppressed");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-memory-pp judge -- --nocapture`
Expected: FAIL — `BrainJudge` doesn't exist. If `entheai_providers::Provider`'s trait method signatures in the `FakeProvider` impl above don't match the real trait (re-check `crates/providers/src/lib.rs:102-135` fresh, since this plan's earlier research of it predates several other changes this session), fix the impl's signatures to match — the test's *intent* (fake `complete()` returning a fixed model name) stays the same regardless of exact signature drift.

- [ ] **Step 3: Implement BrainJudge**

```rust
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use entheai_providers::{ChatMessage, Provider};
use tokio::sync::mpsc;

use crate::frozen::FrozenStore;

pub enum BrainJudgeEvent {
    Woke(String),
}

pub struct BrainJudge {
    provider: Arc<dyn Provider>,
    model: String,
    frozen: FrozenStore,
    cooldown: Duration,
    last_fired_ms: Arc<AtomicI64>,
    tx: mpsc::UnboundedSender<BrainJudgeEvent>,
}

impl BrainJudge {
    pub fn new(
        provider: Arc<dyn Provider>,
        model: String,
        frozen: FrozenStore,
        cooldown: Duration,
    ) -> (Self, mpsc::UnboundedReceiver<BrainJudgeEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self { provider, model, frozen, cooldown, last_fired_ms: Arc::new(AtomicI64::new(0)), tx },
            rx,
        )
    }

    /// Notify the judge of new ambient activity (a tool call summary, a
    /// transcript turn). Fire-and-forget: spawns its own judgment task, never
    /// blocks the caller. Fail-safe by construction — any error/timeout
    /// silently surfaces nothing (see `judge_once`).
    pub async fn notify(&self, activity: &str) {
        if self.frozen.is_empty() {
            return;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let last = self.last_fired_ms.load(Ordering::Relaxed);
        if now_ms - last < self.cooldown.as_millis() as i64 {
            return; // within cooldown — suppressed
        }
        self.last_fired_ms.store(now_ms, Ordering::Relaxed);

        let names: Vec<&str> = self.frozen.nodes().iter().map(|n| n.name.as_str()).collect();
        let prompt = format!(
            "Recent activity: {activity}\n\nAvailable topics: {}\n\n\
             Reply with the single most relevant topic name if one clearly applies, \
             or the word \"none\" if nothing is clearly relevant. Reply with nothing else.",
            names.join(", "),
        );
        let messages = vec![ChatMessage::user(prompt)];
        let provider = Arc::clone(&self.provider);
        let model = self.model.clone();
        let node_names: Vec<String> = self.frozen.nodes().iter().map(|n| n.name.clone()).collect();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let Ok(resp) = tokio::time::timeout(
                Duration::from_secs(5),
                provider.complete(&model, &messages, &[]),
            ).await else { return }; // timeout → surface nothing
            let Ok(resp) = resp else { return }; // provider error → surface nothing
            let answer = resp.content.trim().to_lowercase();
            if let Some(matched) = node_names.iter().find(|n| answer.contains(&n.to_lowercase())) {
                let _ = tx.send(BrainJudgeEvent::Woke(matched.clone()));
            }
        });
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p entheai-memory-pp judge -- --nocapture`
Expected: PASS (3 tests). Note: `notify` spawns a detached task, so the test `await`s `judge.notify(...)` (which returns immediately) then awaits the event on the channel with a timeout — the timeout is what actually waits for the spawned judgment to complete, not `notify` itself.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p entheai-memory-pp --all-targets -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/memory-pp/src/judge.rs crates/memory-pp/src/lib.rs
git commit -m "feat(memory-pp): BrainJudge — event-driven, LLM-judged proactive frozen-node surfacing"
```

---

### Task 5: Wire BrainJudge into the TUI

**Files:**
- Modify: `crates/tui/src/lib.rs`
- Modify: `bin/entheai/src/main.rs`

**Interfaces:**
- Consumes: `BrainJudge`, `BrainJudgeEvent` (Task 4), `entheai_viz::BrainState::wake_frozen` (pre-existing).

- [ ] **Step 1: Construct BrainJudge in bin/entheai and pass it in**

Building a `BrainJudge` needs a `Provider` — reuse whatever the interactive session's own model resolution already produces (re-`grep` how `agent`/`model_id` get built earlier in `main.rs`'s `run()` function to find the already-constructed provider/client, rather than building a second one — check whether the existing `agent: Agent<P>` exposes its inner provider, or whether a fresh lightweight one needs building from `cfg.providers` the same way `resolve` logic elsewhere in `main.rs` already does). Pass the resulting `(BrainJudge, receiver)` pair (or just the `BrainJudge` + let `crates/tui` hold the receiver) into `entheai_tui::run(...)` as new trailing parameters, alongside a chosen cooldown (start with `Duration::from_secs(30)` — a plan-time default per the spec's open question, tune later).

This step is intentionally left as a locate-then-wire step rather than fully prescribed code: which `Provider` instance to reuse depends on `main.rs`'s current agent-construction code, which should be re-read fresh at implementation time rather than assumed from this plan's earlier research (that research focused on `entheai_providers::Provider`'s trait shape, not `main.rs`'s specific construction call site for the interactive path).

- [ ] **Step 2: Feed BrainJudge from the existing ingest points**

In `crates/tui/src/lib.rs`'s event loop, wherever tool results / transcript turns currently would call (post-Task-3) `pp.ingest_tool`/`pp.ingest_transcript` indirectly via `run_task_with_memory` — `BrainJudge` needs its OWN notification, not routed through `PromptProcessor`. Simplest correct wiring: call `judge.notify(&summary).await` directly from the TUI's own `AgentEvent::ToolFinished { name, result }` handling arm (already exists, per Task 3 Step 4's neighboring match arms) — build `summary` as e.g. `format!("used tool {name}: {result}")`, capped to a reasonable length (reuse the existing `cap_bytes` pattern from `crates/memory-pp/src/frozen.rs` or `processor.rs` if one is already `pub(crate)`-visible and reusable, otherwise a short local truncation is fine — this is genuinely a minor detail, not an architectural one).

- [ ] **Step 3: Drain BrainJudge's event channel and wake the brain ring**

In the TUI's main render/poll loop (wherever `events_rx` is already polled each tick, per Task 3), add a similar non-blocking drain of the `BrainJudgeEvent` receiver:

```rust
                        while let Ok(BrainJudgeEvent::Woke(name)) = judge_rx.try_recv() {
                            app.brain.wake_frozen(&name);
                        }
```

- [ ] **Step 4: Build, test, clippy**

```bash
cargo build -p entheai-tui -p entheai 2>&1 | tail -80
cargo test -p entheai-tui -- --nocapture
cargo clippy -p entheai-tui -p entheai --all-targets -- -D warnings
```

- [ ] **Step 5: Manual end-to-end verification (the spec's success criterion)**

Per spec §8: edit a file whose path/content matches a frozen node's domain but do NOT mention that topic in the chat message at all (e.g. touch a `.tf` file via a tool call, then ask an unrelated question) — confirm the `terraform` (or whichever) frozen node's ring glow lights up anyway, proving genuine proactive surfacing (not the user's own words triggering the reactive Slice-1 path).

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/lib.rs bin/entheai/src/main.rs
git commit -m "feat(tui): wire BrainJudge into the interactive loop for proactive surfacing"
```

---

### Task 6: Full workspace gate

**Files:** none — verification only.

- [ ] **Step 1: Full test suite**

```bash
cargo test --workspace 2>&1 | tail -60
```

- [ ] **Step 2: Full clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 3: Re-run both manual verifications**

Task 3 Step 8 (reactive wake, raw-store row proof) and Task 5 Step 5 (proactive wake) together in one real session, confirming both work without interfering with each other (e.g. a reactive wake during the cooldown window doesn't get suppressed by BrainJudge's unrelated cooldown timer — they're independent paths, but worth confirming in practice).

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix: full workspace gate fixes for BRAIN v1"
```

## Self-Review Notes

- **Task 1-3 (Slice 1) are low-risk**: they close gaps using a pre-existing, previously-written recipe (`docs/superpowers/plans/2026-07-19-entheai-memory-v1.md`'s Task 9) that this plan verified against the *current* codebase state and extended with `pp`/frozen-wake wiring the old recipe didn't cover.
- **Task 4 (BrainJudge core) is fully speced with real, existing types** (`entheai_providers::Provider`, `FrozenStore`) — no external framework, no unverified signatures.
- **Task 5 (TUI wiring for BrainJudge) is deliberately less prescriptive** at Step 1 (provider reuse) and Step 2 (exact notify call site) — these depend on reading `bin/entheai/src/main.rs`'s current agent-construction code and the TUI's current `AgentEvent::ToolFinished` handling fresh, which this plan's research didn't do in full (it verified the *existence* of these seams, not their exact current code, consistent with not guessing signatures this plan hasn't checked).
- **Spec coverage**: gap #1 (TUI memory wiring) → Task 3; gap #2 (frozen never called) → Tasks 1-2; proactive surfacing → Tasks 4-5; visual-only surfacing (no chat injection) → Task 2/5 both route through `BrainState::wake_frozen` + the footer pattern, never through `messages`/chat history for the *proactive* path (only the *reactive* Slice-1 path, per spec §3, legitimately also injects into the model's context — that's retrieval, not chat injection, and matches the spec's explicit design).
- **Non-goals respected**: no new frozen-node authoring tooling, no memory review/edit UI, no new crate boundary for `BrainJudge` (lives in `memory-pp` as decided), adk-rust migration untouched.
