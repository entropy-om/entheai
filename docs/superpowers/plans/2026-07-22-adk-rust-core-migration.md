# entheai core agent loop → adk-rust Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `entheai --fanout` (agy/AgyExecutor) for subagent-driven execution of this plan, task-by-task, in place of superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `crates/core`'s hand-rolled `Agent<P: Provider>::run_task`/`run_task_with_memory` loop and `crates/providers` with `adk-rust` v1.0.0 (`adk-agent`/`adk-model`/`adk-tool`/`adk-core`/`adk-runner`/`adk-session`), per `docs/superpowers/specs/2026-07-22-adk-rust-core-migration-design.md`. Big-bang, single PR, no fallback engine — see the spec's §1/§7 for why.

**Architecture:** `crates/core` becomes a thin wrapper (`EntheaiAgent`) around `adk_agent::LlmAgentBuilder` + `adk_runner::Runner` + `adk_session::InMemorySessionService`. Entheai's own `Tool`/`Policy`/`Prompter`/memory types stay unchanged internally; they're bridged into adk-rust via one adapter (`AdkToolAdapter`) and four callbacks (`before_agent`, `after_tool_full`, plus reusing adk's built-in `max_iterations`). `crates/providers` is deleted; model access goes through `adk_model::openai::{OpenAIClient, OpenAIConfig}` (works for OpenAI, osaurus, and any other OpenAI-compatible endpoint via `OpenAIConfig::compatible(api_key, base_url, model)`).

**Tech Stack:** Rust, `adk-rust = "1.0.0"` (pin exact version — `main` branch has already drifted toward an unreleased 2.0.0; do not follow `main`-branch docs/examples), existing entheai conventions (tokio, async-trait, wiremock for HTTP-mocked tests).

## Global Constraints

- **Pin `adk-rust` to version `1.0.0` exactly** in every `Cargo.toml` that adds it (workspace `[workspace.dependencies]` entry: `adk-rust = "1.0.0"`, or the specific sub-crates if depending on them directly — confirm at Task 1 whether the umbrella `adk-rust` crate re-exports `adk-agent`/`adk-model`/`adk-tool`/`adk-core`/`adk-runner`/`adk-session` under feature flags, or whether each must be a direct dependency; the README's manual-install section suggests the umbrella crate with feature tiers `minimal`/`standard`/`enterprise`/`full` — `standard` is very likely the right tier since it includes OpenAI support, tools, and server pieces `minimal` (Gemini-only) lacks. Verify at Task 1 Step 1.).
- **Do not trust `main`-branch adk-rust source, docs, or examples for anything not already verified in this plan.** Every signature in this plan was checked against the `v1.0.0` git tag specifically. If a task needs a signature not already given here, re-verify against `v1.0.0`, not `main`.
- Workspace `rust-version` (`Cargo.toml:10`, currently `"1.80"`) rises to `"1.94"` (Task 9) — this must happen before any adk-rust dependency is added, so early tasks may temporarily fail on CI toolchains older than 1.94 until Task 9 lands; local dev uses whatever rustc is installed (confirmed 1.96 available).
- Every new/modified crate must pass `cargo test -p <crate>` and `cargo clippy -p <crate> -- -D warnings` before its commit step.
- No runtime fallback exists after Task 10 lands (per spec §7) — the final full-workspace gate (Task 10) is the only safety net; every parity test (Task 8) must pass before this plan is considered done.

---

### Task 1: Add adk-rust, spike the wiring end-to-end

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/core/Cargo.toml`
- Create: `crates/core/tests/adk_spike.rs`

**Interfaces:**
- Produces: confirmation that `LlmAgentBuilder` → `Runner` → `InMemorySessionService` → a `#[tool]`-defined function tool → an `OpenAIClient` pointed at a mocked HTTP endpoint round-trips correctly. This de-risks every later task before real adapter code is written.

- [ ] **Step 1: Add the dependency and confirm the feature tier**

In `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
adk-rust = { version = "1.0.0", features = ["standard"] }
```

Run `cargo tree -p entheai-core -i adk-rust 2>&1 | head -30` after Step 2 adds it as a dependency, to confirm which of `adk-core`/`adk-agent`/`adk-model`/`adk-tool`/`adk-runner`/`adk-session` come in as re-exports vs. need to be named directly (the umbrella crate's `lib.rs` re-export list determines the exact import paths used in every later task — e.g. `adk_rust::agent::LlmAgentBuilder` vs. a direct `adk_agent::LlmAgentBuilder` dependency). If the umbrella crate's re-exports don't cover everything cleanly, add the sub-crates directly instead (`adk-core = "1.0.0"`, `adk-agent = "1.0.0"`, `adk-model = "1.0.0"`, `adk-tool = "1.0.0"`, `adk-runner = "1.0.0"`, `adk-session = "1.0.0"`) — whichever resolves with the fewest import-path surprises. Record the decision in a one-line comment above the dependency.

- [ ] **Step 2: Add to crates/core/Cargo.toml**

```toml
[dependencies]
adk-rust = { workspace = true }
```

(Or the sub-crates individually, per Step 1's decision.)

- [ ] **Step 3: Write the spike test**

Create `crates/core/tests/adk_spike.rs`:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use adk_agent::LlmAgentBuilder;
use adk_core::{Agent, Content, Tool};
use adk_model::openai::{OpenAIClient, OpenAIConfig};
use adk_runner::Runner;
use adk_session::{CreateRequest, InMemorySessionService, SessionService};
use adk_tool::FunctionTool;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Deserialize, schemars::JsonSchema)]
struct EchoArgs {
    text: String,
}

async fn echo(args: EchoArgs) -> Result<Value, adk_core::AdkError> {
    Ok(json!({ "echoed": args.text }))
}

#[tokio::test]
async fn spike_llm_agent_runner_session_roundtrip() {
    let server = MockServer::start().await;
    // Non-tool-calling chat-completions response: the model just answers directly.
    // Exact response body shape confirmed against OpenAI-compatible chat/completions
    // format already used by crates/providers' own tests (see
    // crates/providers/src/lib.rs's `complete_handles_plain_text_answer` test for
    // the reference JSON shape) — reuse that same fixture shape here.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "spike-1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "spike ok" },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let config = OpenAIConfig::compatible("no-key", &server.uri(), "spike-model");
    let model = Arc::new(OpenAIClient::new(config).expect("client builds"));

    let tool: Arc<dyn Tool> = Arc::new(
        FunctionTool::new("echo", "Echo text back.", echo).with_parameters_schema::<EchoArgs>(),
    );

    let agent = Arc::new(
        LlmAgentBuilder::new("spike-agent")
            .instruction("You are a test agent.")
            .model(model)
            .tool(tool)
            .max_iterations(5)
            .build()
            .expect("agent builds"),
    );

    let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    sessions
        .create(CreateRequest {
            app_name: "entheai-spike".into(),
            user_id: "spike-user".into(),
            session_id: Some("spike-session".into()),
            state: HashMap::new(),
        })
        .await
        .expect("session creates");

    let runner = Runner::builder()
        .app_name("entheai-spike")
        .agent(agent)
        .session_service(sessions)
        .build()
        .expect("runner builds");

    let mut stream = runner
        .run_str("spike-user", "spike-session", Content::new("user").with_text("say hi"))
        .await
        .expect("run starts");

    let mut final_text = String::new();
    while let Some(ev) = stream.next().await {
        let ev = ev.expect("no stream error");
        if let Some(content) = ev.content() {
            for part in &content.parts {
                if let Some(t) = part.text() {
                    final_text.push_str(t);
                }
            }
        }
    }
    assert!(final_text.contains("spike ok"), "expected mocked reply, got {final_text:?}");
}
```

- [ ] **Step 4: Run it**

Run: `cargo test -p entheai-core --test adk_spike -- --nocapture`

Expected: PASS. If it fails on a type/method mismatch, the failure itself is the valuable output — it means one of the signatures recorded in this plan doesn't match what's actually published under `adk-rust = "1.0.0"`'s re-export surface (as opposed to depending on the sub-crates directly), which Step 1 was meant to catch. Fix the import path (not the underlying type usage) and re-run before proceeding to any other task.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/core/Cargo.toml crates/core/tests/adk_spike.rs Cargo.lock
git commit -m "spike(core): prove adk-rust LlmAgent+Runner+Session wiring against a mocked endpoint"
```

---

### Task 2: AdkToolAdapter — bridge entheai_tools::Tool into adk_core::Tool

**Files:**
- Create: `crates/core/src/adk_tool_adapter.rs`
- Modify: `crates/core/src/lib.rs` (add `mod adk_tool_adapter; pub use adk_tool_adapter::AdkToolAdapter;`)

**Interfaces:**
- Consumes: `entheai_tools::Tool` (`name() -> &str`, `schema() -> Value`, `async fn call(&self, args: Value) -> Result<String, ToolError>`, `tier() -> entheai_permission::Tier`), `entheai_permission::{Policy, Prompter, Decision, Grant}` (all pre-existing, unchanged).
- Produces: `pub struct AdkToolAdapter` implementing `adk_core::Tool`, consumed by Task 4.

- [ ] **Step 1: Write the failing test**

Create `crates/core/src/adk_tool_adapter.rs` with the test module first:

```rust
use std::sync::Arc;

use adk_core::Tool as AdkTool;
use async_trait::async_trait;
use entheai_permission::{Decision, Grant, Policy, Prompter, Tier};
use serde_json::{json, Value};

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool;
    #[async_trait]
    impl entheai_tools::Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn schema(&self) -> Value {
            json!({ "name": "echo", "description": "Echoes input.", "parameters": {} })
        }
        async fn call(&self, args: Value) -> Result<String, entheai_tools::ToolError> {
            Ok(args["text"].as_str().unwrap_or("").to_string())
        }
        fn tier(&self) -> Tier { Tier::Exec }
    }

    struct AlwaysAllow;
    #[async_trait]
    impl Prompter for AlwaysAllow {
        async fn confirm(&mut self, _tool_name: &str, _args_summary: &str) -> Grant { Grant::Allow }
    }

    fn allow_all_policy() -> Policy { Policy::new(true, vec![]) } // yolo = allow everything

    #[tokio::test]
    async fn declaration_matches_inner_schema_verbatim() {
        let adapter = AdkToolAdapter::new(
            Arc::new(EchoTool),
            Arc::new(allow_all_policy()),
            Arc::new(tokio::sync::Mutex::new(AlwaysAllow)),
        );
        assert_eq!(AdkTool::name(&adapter), "echo");
        assert_eq!(AdkTool::declaration(&adapter), json!({
            "name": "echo", "description": "Echoes input.", "parameters": {}
        }));
    }

    #[tokio::test]
    async fn allowed_call_delegates_and_wraps_result() {
        let adapter = AdkToolAdapter::new(
            Arc::new(EchoTool),
            Arc::new(allow_all_policy()),
            Arc::new(tokio::sync::Mutex::new(AlwaysAllow)),
        );
        let ctx = adk_core::testing::noop_tool_context(); // see Step 2 note if this helper doesn't exist
        let out = AdkTool::execute(&adapter, ctx, json!({ "text": "hi" })).await.unwrap();
        assert_eq!(out, json!({ "result": "hi" }));
    }

    #[tokio::test]
    async fn denied_call_returns_error_value_not_err() {
        struct AlwaysDeny;
        #[async_trait]
        impl Prompter for AlwaysDeny {
            async fn confirm(&mut self, _tool_name: &str, _args_summary: &str) -> Grant { Grant::Deny }
        }
        let policy = Policy::new(false, vec![]); // not yolo → Ask path (Exec tier defaults to Ask under non-yolo; confirm via Policy::decide_tiered's actual default table at implementation time, crates/permission/src/lib.rs:142-175)
        let adapter = AdkToolAdapter::new(
            Arc::new(EchoTool),
            Arc::new(policy),
            Arc::new(tokio::sync::Mutex::new(AlwaysDeny)),
        );
        let ctx = adk_core::testing::noop_tool_context();
        let out = AdkTool::execute(&adapter, ctx, json!({ "text": "hi" })).await.unwrap();
        assert!(out["error"].as_str().unwrap().contains("permission denied"));
    }
}
```

**Before running this**, check whether `adk_core::testing::noop_tool_context()` (or an equivalent no-op `Arc<dyn ToolContext>` test helper) actually exists in `adk-core` — it wasn't confirmed in research. If it doesn't exist, `grep -rn "ToolContext" $(cargo metadata --format-version=1 | jq -r '.packages[] | select(.name=="adk-core") | .manifest_path' | xargs dirname)/src` in the vendored source under `~/.cargo/registry` to find a concrete impl to construct one, or write a minimal test-only `struct NoopToolContext;` implementing whatever `ToolContext`/`CallbackContext`/`ReadonlyContext` require (all three traits have defaulted methods per Task research — check how many are actually required with no default before writing the stub).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-core adk_tool_adapter -- --nocapture`
Expected: FAIL — `AdkToolAdapter` doesn't exist yet.

- [ ] **Step 3: Implement AdkToolAdapter**

Above the `#[cfg(test)]` block in the same file:

```rust
pub struct AdkToolAdapter {
    inner: Arc<dyn entheai_tools::Tool>,
    policy: Arc<Policy>,
    prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
}

impl AdkToolAdapter {
    pub fn new(
        inner: Arc<dyn entheai_tools::Tool>,
        policy: Arc<Policy>,
        prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
    ) -> Self {
        Self { inner, policy, prompter }
    }
}

#[async_trait]
impl AdkTool for AdkToolAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        // Unused: `declaration()` below returns entheai's pre-built schema
        // (name+description+parameters already baked in) instead of composing
        // from name()/description()/parameters_schema() like the adk-core
        // default. Real text lives inside that schema's "description" field.
        ""
    }

    fn declaration(&self) -> Value {
        self.inner.schema()
    }

    async fn execute(
        &self,
        _ctx: Arc<dyn adk_core::ToolContext>,
        args: Value,
    ) -> adk_core::Result<Value> {
        let name = self.inner.name();
        let tier = self.inner.tier();
        let allowed = match self.policy.decide_tiered(name, tier) {
            Decision::Allow => true,
            Decision::Deny => false,
            Decision::Ask => {
                let args_summary = args.to_string();
                let mut p = self.prompter.lock().await;
                match p.confirm(name, &args_summary).await {
                    Grant::Deny => false,
                    Grant::Allow => true,
                    Grant::AllowSession => {
                        self.policy.grant_session(name);
                        true
                    }
                }
            }
        };
        if !allowed {
            return Ok(json!({ "error": format!("permission denied for tool '{name}'") }));
        }
        match self.inner.call(args).await {
            Ok(text) => Ok(json!({ "result": text })),
            Err(e) => Err(adk_core::AdkError::tool(e.to_string())),
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p entheai-core adk_tool_adapter -- --nocapture`
Expected: PASS (3 tests).

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p entheai-core -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/adk_tool_adapter.rs crates/core/src/lib.rs
git commit -m "feat(core): AdkToolAdapter bridges entheai Tool+Policy+Prompter into adk_core::Tool"
```

---

### Task 3: Model resolution — "provider/model" config strings → adk Llm client

**Files:**
- Create: `crates/core/src/model_resolve.rs`
- Modify: `crates/core/src/lib.rs` (add `mod model_resolve; pub use model_resolve::resolve_model;`)

**Interfaces:**
- Consumes: `entheai_config::ProviderConfig { base_url: String, api_key_env: Option<String> }` (pre-existing), a `"provider/model"` string (pre-existing convention, e.g. `"osaurus/qwen3-coder"`).
- Produces: `pub fn resolve_model(spec: &str, providers: &HashMap<String, ProviderConfig>) -> anyhow::Result<Arc<dyn adk_core::Llm>>`, consumed by Task 4.

- [ ] **Step 1: Write the failing test**

```rust
use std::collections::HashMap;

use entheai_config::ProviderConfig;

use super::*;

#[test]
fn resolves_provider_slash_model_into_a_client() {
    let mut providers = HashMap::new();
    providers.insert(
        "osaurus".to_string(),
        ProviderConfig { base_url: "http://localhost:8000/v1".to_string(), api_key_env: None },
    );
    let client = resolve_model("osaurus/qwen3-coder", &providers);
    assert!(client.is_ok(), "expected a resolved client: {:?}", client.err());
}

#[test]
fn unknown_provider_errors() {
    let providers = HashMap::new();
    let client = resolve_model("nope/some-model", &providers);
    assert!(client.is_err());
}

#[test]
fn malformed_spec_without_slash_errors() {
    let providers = HashMap::new();
    let client = resolve_model("no-slash-here", &providers);
    assert!(client.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-core model_resolve -- --nocapture`
Expected: FAIL — `resolve_model` doesn't exist.

- [ ] **Step 3: Implement**

```rust
use std::collections::HashMap;
use std::sync::Arc;

use adk_model::openai::{OpenAIClient, OpenAIConfig};
use anyhow::{anyhow, Context};
use entheai_config::ProviderConfig;

/// Resolve a `"<provider>/<model>"` spec (e.g. `"osaurus/qwen3-coder"`) into a
/// live adk-rust model client, using the same `[providers.<name>]` config
/// shape entheai already reads (`base_url` + optional `api_key_env`).
pub fn resolve_model(
    spec: &str,
    providers: &HashMap<String, ProviderConfig>,
) -> anyhow::Result<Arc<dyn adk_core::Llm>> {
    let (provider_name, model_name) = spec
        .split_once('/')
        .ok_or_else(|| anyhow!("model spec {spec:?} must be \"<provider>/<model>\""))?;
    let pc = providers
        .get(provider_name)
        .ok_or_else(|| anyhow!("unknown provider {provider_name:?} in model spec {spec:?}"))?;
    let api_key = match &pc.api_key_env {
        Some(env_var) => std::env::var(env_var)
            .with_context(|| format!("env var {env_var:?} not set for provider {provider_name:?}"))?,
        None => "not-needed".to_string(),
    };
    let config = OpenAIConfig::compatible(&api_key, &pc.base_url, model_name);
    let client = OpenAIClient::new(config)
        .with_context(|| format!("building client for provider {provider_name:?}"))?;
    Ok(Arc::new(client))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p entheai-core model_resolve -- --nocapture`
Expected: PASS. If `adk_core::Llm` isn't the trait `OpenAIClient` actually implements (research named it `Llm` at `adk-core/src/model.rs`, consumed by `LlmAgentBuilder::model(mut self, model: Arc<dyn Llm>)`), fix the trait path here — this is the one signature in this plan sourced from the first (pre-tag-verified) research pass rather than a direct v1.0.0 grep; verify `adk-core/src/model.rs`'s exact trait name against the `v1.0.0` tag if this step fails.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p entheai-core -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/model_resolve.rs crates/core/src/lib.rs
git commit -m "feat(core): resolve provider/model config strings into adk-rust Llm clients"
```

---

### Task 4: EntheaiAgent — the wrapper replacing Agent<P>::run_task

**Files:**
- Modify: `crates/core/src/lib.rs` (replace `Agent<P: Provider>`, `run_task`, `run_task_with_memory`, `stream_turn`, `dispatch_call`, `AgentEvent`, `TokenSink`, `CoreError` with the new wrapper below; keep `DispatchResult`'s successor if still needed internally)

**Interfaces:**
- Consumes: `AdkToolAdapter` (Task 2), `resolve_model` (Task 3), `entheai_tools::ToolRegistry`, `entheai_permission::{Policy, Prompter}`, `entheai_memory::{MemoryRuntime, MemoryScope, ToolEvidence}`, `entheai_memory_pp::PromptProcessor` (all pre-existing).
- Produces: `pub struct EntheaiAgent`, `pub async fn EntheaiAgent::run(&self, user_message: &str) -> anyhow::Result<adk_core::agent::EventStream>` and a memory-aware variant — exact final shape settled in this task, consumed by Task 6 (event wiring) and Task 7 (caller updates).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod entheai_agent_tests {
    use super::*;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn run_returns_final_text_with_no_tool_calls() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "t1", "object": "chat.completion",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "final answer"}, "finish_reason": "stop"}]
            })))
            .mount(&server).await;

        let mut providers = HashMap::new();
        providers.insert("test".to_string(), entheai_config::ProviderConfig {
            base_url: server.uri() + "/v1", api_key_env: None,
        });

        let agent = EntheaiAgent::new(
            "test/model",
            &providers,
            entheai_tools::ToolRegistry::new(),
            std::sync::Arc::new(entheai_permission::Policy::new(true, vec![])),
            std::sync::Arc::new(tokio::sync::Mutex::new(entheai_permission::StdinPrompter)),
            25,
        ).expect("agent builds");

        let text = agent.run_to_text("hello").await.expect("run succeeds");
        assert_eq!(text, "final answer");
    }
}
```

(`run_to_text` is a thin test/CLI-convenience helper added alongside the streaming `run` — collects the event stream into a final string. Real interactive callers use the streaming form directly; see Task 6.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-core entheai_agent_tests -- --nocapture`
Expected: FAIL — `EntheaiAgent` doesn't exist.

- [ ] **Step 3: Implement EntheaiAgent**

```rust
use std::collections::HashMap;
use std::sync::Arc;

use adk_agent::LlmAgentBuilder;
use adk_core::{Agent as _, Content};
use adk_runner::Runner;
use adk_session::{CreateRequest, InMemorySessionService, SessionService};
use entheai_config::ProviderConfig;
use entheai_permission::{Policy, Prompter};
use futures::StreamExt;
use uuid::Uuid;

pub struct EntheaiAgent {
    runner: Runner,
    app_name: String,
}

impl EntheaiAgent {
    pub fn new(
        model_spec: &str,
        providers: &HashMap<String, ProviderConfig>,
        registry: entheai_tools::ToolRegistry,
        policy: Arc<Policy>,
        prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
        max_iterations: u32,
    ) -> anyhow::Result<Self> {
        let model = crate::model_resolve::resolve_model(model_spec, providers)?;

        let mut builder = LlmAgentBuilder::new("entheai")
            .model(model)
            .max_iterations(max_iterations);
        for tool in registry.into_tools() {
            // `ToolRegistry::into_tools(self) -> Vec<Box<dyn entheai_tools::Tool>>` —
            // NEW method needed on ToolRegistry (crates/tools/src/lib.rs); it doesn't
            // exist today (only `register`/`get`/`schemas`). Add it as a small,
            // separate change to crates/tools before this line compiles — a
            // consuming iterator over the registry's tools, since AdkToolAdapter
            // needs owned `Arc<dyn entheai_tools::Tool>` per tool. See Task 4a.
            let adapter = crate::adk_tool_adapter::AdkToolAdapter::new(
                Arc::from(tool),
                Arc::clone(&policy),
                Arc::clone(&prompter),
            );
            builder = builder.tool(Arc::new(adapter));
        }
        let agent: Arc<dyn adk_core::Agent> = Arc::new(builder.build()?);

        let app_name = "entheai".to_string();
        let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let runner = Runner::builder()
            .app_name(app_name.clone())
            .agent(agent)
            .session_service(sessions)
            .build()?;

        Ok(Self { runner, app_name })
    }

    /// Streaming entry point — the interactive TUI and orchestrator's fan-out
    /// coders consume this directly (Task 6/7). Each call is its own fresh
    /// session (entheai doesn't need adk's cross-call session persistence — the
    /// full conversation history is entheai's own concern upstream of this call,
    /// same as `run_task` took `Vec<ChatMessage>` fresh each time. Task 6 confirms
    /// whether session per *task* or per *turn* is right once TUI wiring is done.)
    pub async fn run(&self, user_message: &str) -> anyhow::Result<adk_core::agent::EventStream> {
        let session_id = Uuid::new_v4().to_string();
        let sessions = self.runner.session_service(); // NEW accessor needed if not already public — check adk-runner's Runner for a getter; if absent, keep a second Arc<dyn SessionService> clone alongside `runner` in the struct instead.
        sessions.create(CreateRequest {
            app_name: self.app_name.clone(),
            user_id: "entheai".to_string(),
            session_id: Some(session_id.clone()),
            state: HashMap::new(),
        }).await?;
        let stream = self.runner
            .run_str("entheai", &session_id, Content::new("user").with_text(user_message))
            .await?;
        Ok(stream)
    }

    /// Test/CLI convenience: collect the stream into the final assistant text.
    pub async fn run_to_text(&self, user_message: &str) -> anyhow::Result<String> {
        let mut stream = self.run(user_message).await?;
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if ev.llm_response.turn_complete {
                if let Some(content) = ev.content() {
                    text.clear();
                    for part in &content.parts {
                        if let Some(t) = part.text() {
                            text.push_str(t);
                        }
                    }
                }
            }
        }
        Ok(text)
    }
}
```

**Two things this step's comments flag as needing a small upstream change before it compiles — do them as part of this task, not deferred:**

- [ ] **Step 3a: Add `ToolRegistry::into_tools`**

In `crates/tools/src/lib.rs`, add:

```rust
impl ToolRegistry {
    /// Consume the registry, yielding its tools for wrapping (e.g. into
    /// `adk_core::Tool` adapters, which need owned `Arc`s, not registry-borrowed refs).
    pub fn into_tools(self) -> Vec<Box<dyn Tool>> {
        self.tools.into_values().collect()
    }
}
```

Add a test in the same file's existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn into_tools_yields_all_registered() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(crate::fs::ReadFile::new(std::env::temp_dir())));
    let tools = reg.into_tools();
    assert_eq!(tools.len(), 1);
}
```

Run `cargo test -p entheai-tools into_tools -- --nocapture` (expect PASS) and `cargo clippy -p entheai-tools -- -D warnings` before continuing.

- [ ] **Step 3b: Confirm `Runner::session_service()` accessor**

Check `adk-runner/src/runner.rs` at the `v1.0.0` tag for a public getter back to the `SessionService` the runner was built with (`grep -n "session_service" adk-runner/src/runner.rs` against the vendored source under `~/.cargo/registry/src/`, once Task 1 has pulled the dependency). If no such accessor exists, change `EntheaiAgent` to hold both `runner: Runner` and `sessions: Arc<dyn SessionService>` as separate fields (clone the `Arc` before moving it into `Runner::builder().session_service(...)`, since builders typically take ownership) — this is a two-line struct change, not a redesign.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p entheai-core entheai_agent_tests -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p entheai-core -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/lib.rs crates/tools/src/lib.rs
git commit -m "feat(core): EntheaiAgent wraps adk-rust LlmAgent+Runner, replacing Agent<P>::run_task"
```

---

### Task 5: Memory callbacks — before_agent retrieval injection, after_tool_full evidence recording

**Files:**
- Modify: `crates/core/src/lib.rs` (extend `EntheaiAgent::new` with an optional memory-aware constructor path, or a builder-style `.with_memory(...)` method — pick whichever reads more like the existing `run_task_with_memory` "same function, optional memory" pattern; recommend a separate `EntheaiAgent::new_with_memory(..., memory: Option<Arc<MemoryRuntime>>, pp: Option<Arc<PromptProcessor>>, scope: MemoryScope)` constructor mirroring `run_task_with_memory`'s own "`None` behaves identically to `run_task`" contract, rather than always building both callbacks and no-op'ing them — simpler to reason about and test)

**Interfaces:**
- Consumes: `entheai_memory::{MemoryRuntime, MemoryScope, ToolEvidence}`, `entheai_memory_pp::PromptProcessor` (all pre-existing, unchanged internals — this task only changes *where* they're called from).
- Produces: memory-aware `EntheaiAgent` construction path, consumed by Task 7 (bin/entheai's one-shot path, the only current `run_task_with_memory` caller).

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn before_agent_callback_injects_retrieval_brief_into_history() {
    // Build a MemoryRuntime backed by an in-memory store seeded with one fact,
    // build an EntheaiAgent via new_with_memory, run a query that should match,
    // and assert (via a wiremock request-body inspector, matching the pattern
    // crates/providers' own tests already use for asserting outbound request
    // shape) that the outbound chat-completions request's message list contains
    // the injected brief as a system/prior message before the user's query.
    //
    // NOTE: exact MemoryRuntime test-construction helper (in-memory store setup)
    // should mirror whatever crates/core's PRE-migration test suite already used
    // to build a MemoryRuntime for run_task_with_memory's own tests — reuse that
    // helper verbatim rather than inventing a new one. Locate it by reading the
    // current (pre-Task-4) crates/core/src/lib.rs test module before it's deleted
    // in Task 4 — copy the relevant test-setup helper out first if Task 4 hasn't
    // already preserved it.
    todo!("fill in using the located MemoryRuntime test helper — see note above")
}
```

This step is intentionally left as a locate-then-fill step rather than fully written code: the exact `MemoryRuntime` in-memory test-construction helper lives in `crates/core/src/lib.rs`'s current test module (pre-Task-4), which Task 4 replaces. **Before starting Task 4**, grep `crates/core/src/lib.rs` for `fn memory_runtime_for_test` or similar (the six parity-test names from the spec's §7 all exercise a memory-free path via `run_task`, but earlier memory-v1 work — `docs/superpowers/plans/2026-07-19-entheai-memory-v1.md` — added `run_task_with_memory`'s own tests separately; find and preserve that helper) and copy it into a shared test-support location (`crates/core/src/test_support.rs` or inline in this task's test module) before Task 4 deletes the old file wholesale.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-core before_agent_callback -- --nocapture`
Expected: FAIL (either `todo!()` panics, or `new_with_memory` doesn't exist yet — whichever comes first once Step 1 is filled in with the located helper).

- [ ] **Step 3: Implement the memory callbacks**

```rust
use adk_core::callbacks::BeforeAgentCallback;
use adk_core::CallbackContext;

fn before_agent_retrieval_callback(
    memory: Arc<entheai_memory::MemoryRuntime>,
    pp: Option<Arc<entheai_memory_pp::PromptProcessor>>,
) -> BeforeAgentCallback {
    Box::new(move |ctx: Arc<dyn CallbackContext>| {
        let memory = Arc::clone(&memory);
        let pp = pp.clone();
        Box::pin(async move {
            let user_content = ctx.user_content();
            let user_msg: String = user_content
                .parts
                .iter()
                .filter_map(|p| p.text())
                .collect::<Vec<_>>()
                .join(" ");
            if user_msg.trim().is_empty() {
                return Ok(None); // continue normally, no injection
            }
            let retrieved = match &pp {
                Some(p) => match p.retrieve(&user_msg).await {
                    Ok(Some(brief)) => Ok(Some(brief)),
                    Ok(None) | Err(_) => memory.retrieve_before(&user_msg).await,
                },
                None => memory.retrieve_before(&user_msg).await,
            };
            match retrieved {
                Ok(Some(_ctx_text)) => {
                    // Returning `Some(content)` here SHORT-CIRCUITS the whole
                    // agent run (confirmed: llm_agent.rs's before_callback
                    // handling treats Ok(Some(_)) as "use this as the final
                    // answer, skip the model call entirely") — which is NOT
                    // what injecting retrieval context should do. before_agent
                    // is therefore the WRONG hook for this; injection belongs
                    // in `before_model_callback` instead (runs once per model
                    // call, can rewrite the outgoing `LlmRequest` via
                    // `BeforeModelResult::Continue(request)` without
                    // short-circuiting). Rewrite this function against
                    // `BeforeModelCallback`'s signature
                    // (`Fn(Arc<dyn CallbackContext>, LlmRequest) -> ...
                    // Result<BeforeModelResult>`) before this task is done —
                    // this draft is left in place to document the wrong turn
                    // and why, not as usable code.
                    unimplemented!("rewrite against BeforeModelCallback — see comment above")
                }
                Ok(None) => Ok(None),
                Err(_) => Ok(None), // non-strict fallback; strict-mode Err surfacing TBD at impl time
            }
        })
    })
}
```

**This step deliberately surfaces a real design correction mid-plan**: `before_agent`'s short-circuit semantics (Task-1-research-confirmed) make it wrong for *augmenting* the request — it's for *replacing* the whole response. Re-derive the callback against `BeforeModelCallback` instead:

```rust
use adk_core::callbacks::{BeforeModelCallback, BeforeModelResult};
use adk_core::model::LlmRequest;

fn before_model_retrieval_callback(
    memory: Arc<entheai_memory::MemoryRuntime>,
    pp: Option<Arc<entheai_memory_pp::PromptProcessor>>,
) -> BeforeModelCallback {
    Box::new(move |ctx: Arc<dyn CallbackContext>, mut request: LlmRequest| {
        let memory = Arc::clone(&memory);
        let pp = pp.clone();
        Box::pin(async move {
            let user_msg: String = ctx
                .user_content()
                .parts
                .iter()
                .filter_map(|p| p.text())
                .collect::<Vec<_>>()
                .join(" ");
            if user_msg.trim().is_empty() {
                return Ok(BeforeModelResult::Continue(request));
            }
            let retrieved = match &pp {
                Some(p) => match p.retrieve(&user_msg).await {
                    Ok(Some(brief)) => Ok(Some(brief)),
                    Ok(None) | Err(_) => memory.retrieve_before(&user_msg).await,
                },
                None => memory.retrieve_before(&user_msg).await,
            };
            // NOTE: `LlmRequest`'s exact field for prepending a system/context
            // message wasn't confirmed in research (only `Event`/`Content`/`Part`
            // were). Before this compiles, grep `adk-core/src/model.rs` for
            // `struct LlmRequest` at the v1.0.0 tag and find its message-list
            // field (likely `contents: Vec<Content>` or similar) — insert a new
            // `Content::new("system").with_text(ctx)` at the position mirroring
            // today's `run_task_with_memory` behavior (immediately before the
            // last user message).
            if let Ok(Some(ctx_text)) = retrieved {
                // request.contents.insert(...) — exact call pending the grep above.
                let _ = ctx_text; // placeholder until LlmRequest's shape is confirmed
            }
            Ok(BeforeModelResult::Continue(request))
        })
    })
}
```

- [ ] **Step 3a: Resolve `LlmRequest`'s exact shape before finishing Step 3**

Run against the vendored source once Task 1 pulls the dependency:
```bash
find ~/.cargo/registry/src -path "*adk-core-1.0.0*" -name "model.rs" -exec grep -n "struct LlmRequest" -A 15 {} \;
```
Fill in the exact field name and insertion logic, remove the placeholder `let _ = ctx_text;`, and delete the abandoned `before_agent_retrieval_callback` draft function entirely (kept above only to document why `before_agent` was rejected — do not ship it).

- [ ] **Step 3b: Wire `after_tool_full` for evidence recording**

```rust
use adk_core::callbacks::AfterToolCallbackFull;

fn after_tool_evidence_callback(
    scope: entheai_memory::MemoryScope,
    pp: Option<Arc<entheai_memory_pp::PromptProcessor>>,
) -> AfterToolCallbackFull {
    Box::new(move |_ctx, tool, args, response| {
        let scope = scope.clone();
        let pp = pp.clone();
        Box::pin(async move {
            if let Some(pp) = &pp {
                let ev = entheai_memory::ToolEvidence {
                    call_id: uuid::Uuid::new_v4().to_string(),
                    name: tool.name().to_string(),
                    args: args.to_string(),
                    result: response.to_string(),
                    allowed: !response.get("error").is_some(),
                };
                pp.ingest_tool(&scope, &ev).await;
            }
            Ok(None) // don't override the tool response
        })
    })
}
```

- [ ] **Step 4: Wire both callbacks into `EntheaiAgent::new_with_memory`**

Add the constructor variant, building on Task 4's `new`:

```rust
impl EntheaiAgent {
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_memory(
        model_spec: &str,
        providers: &HashMap<String, ProviderConfig>,
        registry: entheai_tools::ToolRegistry,
        policy: Arc<Policy>,
        prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
        max_iterations: u32,
        memory: Option<Arc<entheai_memory::MemoryRuntime>>,
        pp: Option<Arc<entheai_memory_pp::PromptProcessor>>,
        scope: entheai_memory::MemoryScope,
    ) -> anyhow::Result<Self> {
        // Same body as `new`, but if `memory.is_some()`, add
        // `.before_model_callback(before_model_retrieval_callback(mem, pp.clone()))`
        // and `.after_tool_callback_full(after_tool_evidence_callback(scope, pp))`
        // to the builder before `.build()`. `None` → identical to `new`, matching
        // `run_task_with_memory`'s "None behaves identically to run_task" contract.
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p entheai-core before_agent_callback -- --nocapture` (rename the test to `before_model_callback_injects_retrieval_brief` once Step 3's correction lands, matching the actual hook used)
Expected: PASS.

- [ ] **Step 6: Clippy**

Run: `cargo clippy -p entheai-core -- -D warnings`

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/lib.rs
git commit -m "feat(core): wire memory retrieval (before_model) and tool evidence (after_tool_full) callbacks"
```

---

### Task 6: Event stream — TUI wiring off AgentEvent onto adk's native Event

**Files:**
- Modify: `crates/tui/src/lib.rs` (the `AgentEvent` match arms feeding the TUI's live "thinking"/tool-status rendering — locate via `grep -n "AgentEvent::" crates/tui/src/lib.rs`, expected near the existing call site at line 732 and wherever `event_tx`/`event_rx` are declared)

**Interfaces:**
- Consumes: `adk_core::agent::EventStream` (`Stream<Item = Result<adk_core::Event>>`) from `EntheaiAgent::run` (Task 4).
- Produces: TUI rendering unchanged from a user's perspective (spec §9 success criterion), driven by matching on `Event`'s fields directly instead of an `AgentEvent` enum.

- [ ] **Step 1: Locate every AgentEvent match arm**

Run: `grep -n "AgentEvent::" crates/tui/src/lib.rs`

For each hit, note what UI behavior it drives (e.g. `Thinking` → show a spinner; `Token(t)` → append `t` to the live response buffer; `ToolStarted`/`ToolFinished` → render tool-call status lines). This mapping is required before writing the replacement match — do not guess it from memory of this session's earlier read; re-grep live, since Task 4/5 changes may have shifted line numbers.

- [ ] **Step 2: Write the translation**

Replace the `AgentEvent` consumption loop with one matching on the real `adk_core::Event` shape (confirmed structure, Task 1's research):

```rust
while let Some(ev) = stream.next().await {
    let ev = match ev {
        Ok(e) => e,
        Err(e) => { /* surface e, same as today's CoreError path */ continue; }
    };
    if !ev.llm_response.partial {
        // A non-partial event with tool calls = "tool about to run"; with
        // content and turn_complete = final answer. Exact boundary between
        // "thinking" (no content yet) and "token" (partial content) is
        // `ev.llm_response.partial` itself — partial=true events are streamed
        // token deltas, matching today's `AgentEvent::Token`.
    }
    if ev.llm_response.partial {
        if let Some(content) = ev.content() {
            for part in &content.parts {
                if let Some(t) = part.text() {
                    // today's AgentEvent::Token(t) equivalent
                }
            }
        }
    }
    for call in ev.tool_calls() {
        // today's AgentEvent::ToolStarted { name, args } equivalent — exact
        // ToolCallView field names not yet confirmed; grep
        // `adk-core/src/event.rs`'s `ToolCallView` struct at v1.0.0 before
        // writing the field accesses here.
    }
    for result in ev.tool_results() {
        // today's AgentEvent::ToolFinished { name, result } equivalent — same
        // caveat, grep `ToolResultView`'s fields first.
    }
}
```

- [ ] **Step 2a: Confirm ToolCallView/ToolResultView field names**

```bash
find ~/.cargo/registry/src -path "*adk-core-1.0.0*" -name "event.rs" -exec grep -n "struct ToolCallView\|struct ToolResultView" -A 8 {} \;
```
Fill in the real field accesses in Step 2 before this compiles.

- [ ] **Step 3: Run the TUI's existing tests**

Run: `cargo test -p entheai-tui -- --nocapture`
Expected: PASS — TUI-level tests should assert on rendered output, not on `AgentEvent` variants directly, so this change should be internally contained. If any test does assert on `AgentEvent` directly, update it to assert on rendered output instead (matching the intent, not the removed type).

- [ ] **Step 4: Clippy**

Run: `cargo clippy -p entheai-tui -- -D warnings`

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): consume adk_core::Event directly, retire AgentEvent"
```

---

### Task 7: Update callers — orchestrator, bin/entheai; delete crates/providers

**Files:**
- Modify: `crates/orchestrator/src/lib.rs` (3 call sites: lines 285, 300, 422 — re-grep first, Task 4/5/6 may shift them)
- Modify: `bin/entheai/src/main.rs` (1 call site: line 305)
- Modify: `crates/tui/src/lib.rs` (1 call site: line 732, likely already touched by Task 6)
- Delete: `crates/providers/` entirely
- Modify: `Cargo.toml` (remove `crates/providers` from `members`)
- Modify: every `Cargo.toml` depending on `entheai-providers` (`crates/orchestrator`, `crates/tui`, `crates/router`, `bin/entheai`, `crates/memory-pp` — re-grep `grep -rl entheai-providers crates bin` first)

**Interfaces:**
- Consumes: `EntheaiAgent::{new, new_with_memory, run, run_to_text}` (Tasks 4-5).
- Produces: a working `cargo build --workspace` with zero `entheai-providers` references anywhere.

- [ ] **Step 1: Re-grep every call site fresh**

```bash
grep -rn "entheai_providers\|Agent::new(\|\.run_task(\|\.run_task_with_memory(" crates bin 2>/dev/null
```

- [ ] **Step 2: Update crates/orchestrator's three call sites**

Each currently builds an `Agent<P>` then calls `.run_task(...)`. Replace with `EntheaiAgent::new(...)` + `.run_to_text(...)` (fan-out coders don't need live streaming — they run headless and capture a final string, matching `CoderRun.output: String`). Exact replacement code depends on what each of the 3 call sites currently threads through as `messages`/`registry`/`policy`/`prompter` — read each site fresh (Step 1) and adapt in place; this is mechanical substitution, not a redesign, since `EntheaiAgent`'s constructor takes the same conceptual inputs (`ToolRegistry`, `Policy`, `Prompter`) as `Agent::new` + `run_task`'s parameters combined.

- [ ] **Step 3: Update bin/entheai's one call site**

Line 305's `.run_task_with_memory(...)` becomes `EntheaiAgent::new_with_memory(...)` + `.run(...)` (the CLI's one-shot path streams to stdout the same way `crates/tui` does post-Task-6 — reuse the same `Event`-consuming loop pattern Task 6 wrote, factored into a small shared helper in `crates/core` if the duplication is more than ~15 lines, per this codebase's existing DRY conventions).

- [ ] **Step 4: Delete crates/providers**

```bash
git rm -r crates/providers
```

Remove `"crates/providers"` from `Cargo.toml`'s `members` list. Remove `entheai-providers = { path = "../providers" }` from every dependent `Cargo.toml` found in Step 1's grep (re-run `grep -rl entheai-providers crates bin --include=Cargo.toml` to be sure none are missed).

- [ ] **Step 5: Build the whole workspace**

Run: `cargo build --workspace 2>&1 | tail -100`
Expected: clean build. Fix compile errors one crate at a time, innermost dependency first (`crates/core` → `crates/orchestrator`/`crates/tui` → `bin/entheai`).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(core): repoint orchestrator/tui/bin callers to EntheaiAgent, delete crates/providers"
```

---

### Task 8: Port the six parity tests

**Files:**
- Create: `crates/core/tests/parity.rs` (or add to `crates/core/src/lib.rs`'s test module, matching whichever location Task 4 left the bulk of `EntheaiAgent`'s own tests in)

**Interfaces:**
- Consumes: `EntheaiAgent` (Tasks 4-5), `wiremock` (existing workspace dev-dependency).

- [ ] **Step 1: Port each test, same scenario and assertion, against EntheaiAgent**

For each of the six tests named in the spec (§7) — `run_task_dispatches_tool_then_returns_final_answer`, `run_task_caps_runaway_tool_loops`, `run_task_emits_thinking_and_tool_events`, `run_task_feeds_back_permission_denied_tool_result`, `run_task_feeds_back_unknown_tool_error`, `run_task_feeds_back_bad_json_args_error` — read the ORIGINAL test body (recover it from git history: `git show <commit-before-Task-4>:crates/core/src/lib.rs` and find the test by name) and rewrite it against `EntheaiAgent`'s API, preserving the exact scenario (same mocked model responses, same tool setup, same expected outcome). Do this one test at a time, running it after each port:

```bash
cargo test -p entheai-core run_task_dispatches_tool_then_returns_final_answer -- --nocapture
```

For `run_task_caps_runaway_tool_loops` specifically: confirm `LlmAgentBuilder::max_iterations` actually enforces the cap the same way (research confirmed `llm_agent.rs:1576` raises an error past `max_iterations` — assert on that error surfacing through `EntheaiAgent::run`/`run_to_text` as an `Err`, same as today's `CoreError::MaxTurnsExceeded`).

For `run_task_feeds_back_permission_denied_tool_result`: this is now exercised at the `AdkToolAdapter` level (Task 2 already has a version of this — `denied_call_returns_error_value_not_err`). Confirm this test's scenario is a superset (full agent loop, not just the adapter in isolation) and keep both; they test different layers.

- [ ] **Step 2: Run all six**

```bash
cargo test -p entheai-core parity -- --nocapture
```
Expected: PASS, 6/6.

- [ ] **Step 3: Clippy**

```bash
cargo clippy -p entheai-core -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/tests/parity.rs
git commit -m "test(core): port all six run_task parity tests to EntheaiAgent"
```

---

### Task 9: Bump workspace rust-version, update cargo-dist toolchain

**Files:**
- Modify: `Cargo.toml` (`rust-version = "1.80"` → `"1.94"`, line 10)
- Modify: whatever `cargo-dist`/CI config pins a Rust toolchain version (`grep -rn "1\.80\|rust-version\|rust-toolchain" .github/workflows/ Cargo.toml 2>/dev/null` to find every reference)

**Interfaces:** none — config-only change.

- [ ] **Step 1: Update the workspace rust-version**

In `Cargo.toml`:
```toml
rust-version = "1.94"
```

- [ ] **Step 2: Find and update every other toolchain pin**

```bash
grep -rn "1\.80\|rust-toolchain" .github/workflows/*.yml Cargo.toml rust-toolchain.toml 2>/dev/null
```
Update each hit consistently to `1.94` (or a `rust-toolchain.toml` `channel = "1.94"` / `"stable"` if that's the existing convention — match whatever's already there).

- [ ] **Step 3: Verify locally**

```bash
rustc --version   # confirm >= 1.94 locally (already 1.96 per Task 1's environment check)
cargo build --workspace
```

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml .github/workflows/ rust-toolchain.toml 2>/dev/null
git commit -m "chore: bump workspace rust-version to 1.94 for adk-rust"
```

---

### Task 10: Full workspace gate

**Files:** none — verification only.

- [ ] **Step 1: Full test suite**

```bash
cargo test --workspace 2>&1 | tail -60
```
Expected: all pass, including all 6 ported parity tests (Task 8), `AdkToolAdapter`'s tests (Task 2), `resolve_model`'s tests (Task 3), the memory callback test (Task 5), and every pre-existing test in every other crate (no incidental breakage).

- [ ] **Step 2: Full clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 3: Manual smoke check against a real osaurus instance**

Per `docs/osaurus-setup.md`, start a real local osaurus server, point `entheai.toml` at it, and run one interactive TUI session confirming: streaming tokens render live, at least one tool call executes and its result renders, a permission-`Ask`-tier tool prompts and responds correctly to both allow and deny. This is the one step in the whole plan that can't be asserted by an automated test — it's the final human check before calling the migration done, matching spec §9's "TUI's live event rendering... works unchanged from a user's perspective."

- [ ] **Step 4: Final commit (if Steps 1-3 required any fixes)**

```bash
git add -A
git commit -m "fix: full workspace gate fixes for adk-rust migration"
```

---

## Self-Review Notes

- **Task granularity was calibrated for a large external-framework migration**, not maximal micro-decomposition: Tasks 2-8 each bundle several bite-sized steps under one "unit that carries its own test cycle" (per the skill's Task Right-Sizing principle) rather than splitting further, because the real risk here is signature mismatches against an unfamiliar library, not step size.
- **Every signature in Tasks 1-4 was verified against the `v1.0.0` git tag** across two research passes (see this plan's Global Constraints). Task 5 (memory callbacks) and Task 6 (event field names) contain a small number of explicitly-marked "confirm before this compiles" sub-steps for the handful of items neither research pass nailed down (`LlmRequest`'s exact field, `ToolCallView`/`ToolResultView`'s field names, `Runner::session_service()`'s existence) — these are called out individually rather than guessed, per the plan's own "no placeholders" requirement; each has a concrete `grep`/`find` command to resolve it against vendored source once Task 1 pulls the dependency.
- **Task 5 intentionally documents a wrong turn** (the `before_agent` short-circuit draft) rather than silently only showing the corrected `before_model` version — this is preserved because it's exactly the kind of mistake an implementer unfamiliar with adk-rust's callback semantics would otherwise make, and the research already paid the cost of discovering why it's wrong.
- **Spec coverage check**: architecture (§4) → Tasks 1-4; data flow (§5) → Tasks 4-6; error handling (§6, permission denial non-fatal + max-turns cap) → Task 2 (denial) + Task 8 (cap parity test); testing (§7) → Task 8 + Task 10; success criteria (§9) → Task 10 Step 3 (TUI parity) + Task 7 Step 4 (providers deleted) + Task 4 (no hand-rolled loop) + Task 10 Steps 1-2 (clean test/clippy).
- **Non-goals from the spec (§8) are respected**: no task touches `crates/tools`/`crates/mcp` internals beyond the one adapter, `crates/orchestrator`'s `WorkerPool` mechanics are untouched (only its 3 call sites into the agent loop change), and no adk-rust feature beyond agent/model/tool/runner/session (RAG, payments, realtime, browser, AWP) is adopted.
