# entheai Config Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lift the HIGH/MED hardcoded knobs into `entheai.toml` with sane defaults, give the orchestrator a hardcoded-strong model default + an authored system prompt, and add the missing provider request timeout/sampling/retry.

**Architecture:** `crates/config` grows the structs + defaults (Task 1 — the foundation everything depends on). Low-level crates gain **non-breaking builders** (`Agent::with_max_turns`, `OpenAiCompatProvider::with_inference`) so they stay self-contained and green on their own. `router::build_agent` — the factory that builds both provider and `Agent` — is the single wiring point that threads config into them. The remaining crates each swap one `const` for a threaded config value.

**Tech Stack:** Rust, `serde`/`toml` (config), `reqwest` (provider), `tokio` (timeouts). No new deps.

---

> ## Multi-session hazard — READ FIRST
> `crates/config/src/lib.rs`, `crates/tui/src/lib.rs`, and `bin/entheai/src/main.rs` are **hot/shared** (concurrent sessions edit them). Every commit: **scoped, explicit-pathspec** (`git commit -m "..." -- <paths>`; `git add <path>` first for any new file) to dodge the auto-stager; **push immediately**; on non-FF `git pull --rebase origin main` (stash only `.repowise/wiki.db` if it blocks). Never `git add -A`/`.`, never `git reset --hard`. New `const`s go at the top of their module — if a rebase conflicts on a hot file, re-read and re-apply; if it conflicts irreconcilably, abort and escalate.
>
> **Task order matters:** Task 1 (config) first — every consumer reads its fields. Tasks 2–3 (providers/core builders) before Task 4 (router wiring). Tasks 5–11 are independent of each other and can run in any order after Task 1.

## File structure

| File | Change |
|---|---|
| `crates/config/src/lib.rs` | +`InferenceConfig`, `ToolsConfig`, `PermissionConfig`, `McpDefaultsConfig`, `RadioConfig`, `TelemetryConfig`; extend `RouterConfig`, `MemoryConfig`, `VizConfig`, `CompanionConfig`; add fields to `Config` (Task 1) |
| `crates/providers/src/lib.rs` | +`InferenceSettings` + `OpenAiCompatProvider::with_inference` + apply timeout/sampling/retry in `post_chat` (Task 2) |
| `crates/core/src/lib.rs` | `Agent` gains `max_turns` + `with_max_turns`; both loops use it (Task 3) |
| `crates/router/src/lib.rs` | `DEFAULT_ORCHESTRATOR` + `DEFAULT_ORCHESTRATOR_PROMPT` consts; `orchestrator_model` fallback; `orchestrator_system_prompt`; `build_agent` wires `max_turns`+inference (Task 4) |
| `crates/tools/src/shell.rs`,`search.rs` | `RunShell`/`Search` take timeout/caps (Task 5) |
| `crates/orchestrator/src/lib.rs` + `bin` | fan-out policy from `[permission]`; single-agent `cli.yolo||config` (Task 6) |
| `crates/memory/src/embed.rs` | `Embedder::new` takes a timeout (Task 7) |
| `crates/tui/src/lib.rs` | tick + pane caps from `[viz]` (Task 8) |
| `crates/companion` + `bin` | port + fps from `[companion]` (Task 9) |
| `crates/radio/src/lib.rs` | download timeout from config (Task 10) |
| `bin/entheai/src/main.rs` | MCP spawn timeout, Sentry DSN, single-agent orchestrator fallback (Task 11) |

---

## Task 1: config core — all new/extended structs + defaults

**Files:** Modify `crates/config/src/lib.rs`

The file already has `Config` (with `router/agents/fanout/mcp/skills/memory/viz/companion` fields) and the `#[serde(default = "…")]` + `impl Default` pattern (see `MemoryConfig`/`VizConfig`). Mirror it exactly.

- [ ] **Step 1: Write the failing test** (add to the `#[cfg(test)] mod tests`):

```rust
#[test]
fn refactor_config_defaults() {
    let cfg = Config::from_toml_str("").unwrap();
    // router
    assert_eq!(cfg.router.max_turns, 25);
    assert!(cfg.router.orchestrator_prompt.is_none());
    assert!(cfg.router.orchestrator_prompt_append.is_none());
    // inference
    assert_eq!(cfg.inference.request_timeout_secs, 120);
    assert!(cfg.inference.max_tokens.is_none());
    assert!(cfg.inference.temperature.is_none());
    assert_eq!(cfg.inference.retries, 2);
    // tools
    assert_eq!(cfg.tools.shell_timeout_secs, 120);
    assert_eq!(cfg.tools.shell_output_cap, 100_000);
    assert_eq!(cfg.tools.search_max_results, 200);
    // permission
    assert!(!cfg.permission.yolo);
    assert!(cfg.permission.allowlist.is_empty());
    assert!(cfg.permission.fanout_auto_approve);
    // mcp_defaults / memory / viz / companion / radio / telemetry
    assert_eq!(cfg.mcp_defaults.spawn_timeout_secs, 10);
    assert_eq!(cfg.memory.embed_timeout_secs, 30);
    assert_eq!(cfg.viz.tick_ms, 90);
    assert_eq!(cfg.viz.plan_rows_cap, 8);
    assert_eq!(cfg.viz.swarm_rows_cap, 8);
    assert_eq!(cfg.companion.port, 9876);
    assert_eq!(cfg.companion.fps, 24.0);
    assert_eq!(cfg.radio.download_timeout_secs, 300);
    assert!(cfg.telemetry.sentry_dsn.is_none());
}

#[test]
fn refactor_config_overrides_parse() {
    let cfg = Config::from_toml_str(
        "[router]\nmax_turns = 10\n[inference]\nrequest_timeout_secs = 5\nmax_tokens = 2048\ntemperature = 0.2\n[permission]\nfanout_auto_approve = false\n[viz]\ntick_ms = 33\n",
    )
    .unwrap();
    assert_eq!(cfg.router.max_turns, 10);
    assert_eq!(cfg.inference.request_timeout_secs, 5);
    assert_eq!(cfg.inference.max_tokens, Some(2048));
    assert_eq!(cfg.inference.temperature, Some(0.2));
    assert!(!cfg.permission.fanout_auto_approve);
    assert_eq!(cfg.viz.tick_ms, 33);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-config refactor_config_defaults`
Expected: FAIL — unknown fields (`max_turns`, `inference`, etc.).

- [ ] **Step 3: Add the fields to `Config`** (next to the existing `#[serde(default)]` sub-configs):

```rust
    #[serde(default)]
    pub inference: InferenceConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub permission: PermissionConfig,
    #[serde(default)]
    pub mcp_defaults: McpDefaultsConfig,
    #[serde(default)]
    pub radio: RadioConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
```

- [ ] **Step 4: Extend `RouterConfig`, `MemoryConfig`, `VizConfig`, `CompanionConfig`.**

`RouterConfig` — add fields + defaults (it already has `orchestrator: Option<String>`, `max_parallel`):
```rust
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    #[serde(default)]
    pub orchestrator_prompt: Option<String>,
    #[serde(default)]
    pub orchestrator_prompt_append: Option<String>,
```
Add `fn default_max_turns() -> usize { 25 }`, and set these three in `impl Default for RouterConfig`.

`MemoryConfig` — add `#[serde(default = "default_embed_timeout_secs")] pub embed_timeout_secs: u64,` + `fn default_embed_timeout_secs() -> u64 { 30 }` + set it in `impl Default`.

`VizConfig` — add:
```rust
    #[serde(default = "default_viz_tick_ms")]
    pub tick_ms: u64,
    #[serde(default = "default_viz_plan_rows_cap")]
    pub plan_rows_cap: u16,
    #[serde(default = "default_viz_swarm_rows_cap")]
    pub swarm_rows_cap: u16,
```
+ `fn default_viz_tick_ms() -> u64 { 90 }`, `fn default_viz_plan_rows_cap() -> u16 { 8 }`, `fn default_viz_swarm_rows_cap() -> u16 { 8 }` + set in `impl Default for VizConfig`.

`CompanionConfig` — add `port: u16` (default 9876) + `fps: f64` (default 24.0) with `default_companion_port`/`default_companion_fps` fns + set in its `impl Default`.

- [ ] **Step 5: Add the new config structs** (place near the others; all derive `Debug, Clone, Deserialize` and have an `impl Default`):

```rust
/// Provider request defaults (applied to every LLM call).
#[derive(Debug, Clone, Deserialize)]
pub struct InferenceConfig {
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default = "default_retries")]
    pub retries: u32,
}
fn default_request_timeout_secs() -> u64 { 120 }
fn default_retries() -> u32 { 2 }
impl Default for InferenceConfig {
    fn default() -> Self {
        Self { request_timeout_secs: default_request_timeout_secs(), max_tokens: None, temperature: None, retries: default_retries() }
    }
}

/// Built-in tool caps.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolsConfig {
    #[serde(default = "default_shell_timeout_secs")]
    pub shell_timeout_secs: u64,
    #[serde(default = "default_shell_output_cap")]
    pub shell_output_cap: usize,
    #[serde(default = "default_search_max_results")]
    pub search_max_results: usize,
}
fn default_shell_timeout_secs() -> u64 { 120 }
fn default_shell_output_cap() -> usize { 100_000 }
fn default_search_max_results() -> usize { 200 }
impl Default for ToolsConfig {
    fn default() -> Self {
        Self { shell_timeout_secs: default_shell_timeout_secs(), shell_output_cap: default_shell_output_cap(), search_max_results: default_search_max_results() }
    }
}

/// Permission policy defaults.
#[derive(Debug, Clone, Deserialize)]
pub struct PermissionConfig {
    #[serde(default)]
    pub yolo: bool,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default = "default_fanout_auto_approve")]
    pub fanout_auto_approve: bool,
}
fn default_fanout_auto_approve() -> bool { true }
impl Default for PermissionConfig {
    fn default() -> Self {
        Self { yolo: false, allowlist: Vec::new(), fanout_auto_approve: default_fanout_auto_approve() }
    }
}

/// Cross-cutting MCP settings (siblings of the per-server `[mcp.<name>]` map).
#[derive(Debug, Clone, Deserialize)]
pub struct McpDefaultsConfig {
    #[serde(default = "default_mcp_spawn_timeout_secs")]
    pub spawn_timeout_secs: u64,
}
fn default_mcp_spawn_timeout_secs() -> u64 { 10 }
impl Default for McpDefaultsConfig {
    fn default() -> Self { Self { spawn_timeout_secs: default_mcp_spawn_timeout_secs() } }
}

/// Radio (background music) settings.
#[derive(Debug, Clone, Deserialize)]
pub struct RadioConfig {
    #[serde(default = "default_radio_download_timeout_secs")]
    pub download_timeout_secs: u64,
}
fn default_radio_download_timeout_secs() -> u64 { 300 }
impl Default for RadioConfig {
    fn default() -> Self { Self { download_timeout_secs: default_radio_download_timeout_secs() } }
}

/// Telemetry / crash reporting.
#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub sentry_dsn: Option<String>,
}
impl Default for TelemetryConfig {
    fn default() -> Self { Self { sentry_dsn: None } }
}
```

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test -p entheai-config refactor_config_defaults refactor_config_overrides_parse`
Expected: PASS. Also run the full `cargo test -p entheai-config` — the existing `memory_config_defaults`/`viz_config_defaults` tests must still pass (you only ADDED fields).

- [ ] **Step 7: Gate + commit**

`cargo clippy -p entheai-config -- -D warnings` → clean. `cargo fmt -p entheai-config`.
```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): configurable knobs — inference/tools/permission/mcp_defaults/radio/telemetry + router/memory/viz/companion extensions" -- crates/config/src/lib.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 2: providers — request timeout + sampling + retry (`with_inference`)

**Files:** Modify `crates/providers/src/lib.rs`

Today `OpenAiCompatProvider::new` builds `reqwest::Client::new()` (NO timeout) and `post_chat` sends the body as-is (no `max_tokens`/`temperature`, no retry). Add a non-breaking `with_inference` builder + apply in `post_chat`. `crates/providers` does NOT depend on `crates/config`, so define a local `InferenceSettings`.

- [ ] **Step 1: Write the failing test** (add to the `#[cfg(test)] mod tests`; the crate already uses `wiremock`):

```rust
#[tokio::test]
async fn request_timeout_fires() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(2)))
        .mount(&server)
        .await;
    let provider = OpenAiCompatProvider::new(server.uri(), None)
        .with_inference(InferenceSettings { request_timeout: std::time::Duration::from_millis(200), max_tokens: None, temperature: None, retries: 0 });
    let err = provider.complete("m", vec![ChatMessage::user("hi")], vec![]).await.unwrap_err();
    // A timeout surfaces as an Unreachable (reqwest transport error), not a Status.
    assert!(matches!(err, ProviderError::Unreachable { .. }), "got {err:?}");
}

#[tokio::test]
async fn sampling_params_sent_when_set() {
    use wiremock::matchers::{method, body_partial_json};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(serde_json::json!({"max_tokens": 512, "temperature": 0.1})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "ok"}}]
        })))
        .mount(&server)
        .await;
    let provider = OpenAiCompatProvider::new(server.uri(), None)
        .with_inference(InferenceSettings { request_timeout: std::time::Duration::from_secs(30), max_tokens: Some(512), temperature: Some(0.1), retries: 0 });
    let resp = provider.complete("m", vec![ChatMessage::user("hi")], vec![]).await.unwrap();
    assert_eq!(resp.content, "ok"); // the body_partial_json matcher enforces the sampling params were sent
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-providers request_timeout_fires sampling_params_sent_when_set`
Expected: FAIL — `cannot find struct InferenceSettings` / `no method with_inference`.

- [ ] **Step 3: Implement.** Add fields + the `InferenceSettings` struct + builder, and apply in `post_chat`.

Extend the struct + `new` (keep `new` defaulting to no-timeout/no-sampling/no-retry so all existing callers are unchanged):
```rust
pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    retries: u32,
}

/// Provider request settings (mapped from `entheai_config::InferenceConfig` by the router).
#[derive(Debug, Clone)]
pub struct InferenceSettings {
    pub request_timeout: std::time::Duration,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub retries: u32,
}

impl OpenAiCompatProvider {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key,
            max_tokens: None,
            temperature: None,
            retries: 0,
        }
    }

    /// Apply provider request settings: rebuilds the client with a request
    /// timeout and records sampling + retry policy. Non-breaking (opt-in).
    pub fn with_inference(mut self, s: InferenceSettings) -> Self {
        self.client = reqwest::Client::builder()
            .timeout(s.request_timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        self.max_tokens = s.max_tokens;
        self.temperature = s.temperature;
        self.retries = s.retries;
        self
    }
}
```

In `post_chat`, inject sampling and wrap send+status in a retry loop:
```rust
    async fn post_chat(&self, mut body: serde_json::Value) -> Result<reqwest::Response, ProviderError> {
        if let Some(mt) = self.max_tokens {
            body["max_tokens"] = serde_json::json!(mt);
        }
        if let Some(t) = self.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut attempt = 0;
        loop {
            let mut req = self.client.post(&url).json(&body);
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            let result = async {
                let resp = req.send().await.map_err(|source| ProviderError::Unreachable { url: url.clone(), source })?;
                let status = resp.status();
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(ProviderError::Status { status: status.as_u16(), body });
                }
                Ok(resp)
            }
            .await;
            match result {
                Ok(resp) => return Ok(resp),
                // Retry transport errors and 5xx; never retry 4xx (client error).
                Err(e) if attempt < self.retries && is_retryable(&e) => {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(200 * (1 << (attempt - 1)))).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
```
Add a free fn:
```rust
fn is_retryable(e: &ProviderError) -> bool {
    matches!(e, ProviderError::Unreachable { .. })
        || matches!(e, ProviderError::Status { status, .. } if *status >= 500)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-providers` → all pass (existing streaming/tool-call tests + the 2 new). Note: `body["max_tokens"] = …` on a `serde_json::Value::Object` is valid; `post_chat` now takes `mut body`.

- [ ] **Step 5: Gate + commit**

`cargo clippy -p entheai-providers -- -D warnings` → clean. `cargo fmt -p entheai-providers`.
```bash
git add crates/providers/src/lib.rs
git commit -m "feat(providers): request timeout + max_tokens/temperature + retry via with_inference" -- crates/providers/src/lib.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 3: core — `Agent` max_turns

**Files:** Modify `crates/core/src/lib.rs`

`run_task` (~line 115) and `run_task_with_memory` (~line 178) both use `const MAX_TURNS: usize = 25`. Give `Agent` a `max_turns` field (default 25) + a `with_max_turns` builder; both loops use `self.max_turns`.

- [ ] **Step 1: Write the failing test** (add to core's `#[cfg(test)] mod tests`; the crate has a recording/mock provider harness — mirror an existing `run_task` test that uses it):

```rust
#[tokio::test]
async fn max_turns_caps_the_loop() {
    // A provider that ALWAYS returns a tool call would loop forever; max_turns=1
    // must stop after one dispatch round with MaxTurnsExceeded.
    let provider = /* the always-calls-a-tool recording provider used by existing tests */;
    let agent = Agent::new(provider, "m".to_string()).with_max_turns(1);
    let registry = /* a registry with the tool the provider calls */;
    let policy = /* yolo policy from the test helpers */;
    let mut prompter = /* AutoAllow/test prompter */;
    let err = agent
        .run_task(vec![ChatMessage::user("go")], &registry, &policy, &mut prompter, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::MaxTurnsExceeded(1)));
}
```
(Use the exact harness types the neighboring `run_task` tests already use — copy their provider/registry/policy/prompter setup; only add `.with_max_turns(1)` and assert `MaxTurnsExceeded(1)`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-core max_turns_caps_the_loop`
Expected: FAIL — `no method named with_max_turns`.

- [ ] **Step 3: Implement.** Add a `max_turns` field to `Agent` (find the `struct Agent<P>` definition and its `new`):
```rust
    // in `struct Agent<P>`
    max_turns: usize,
    // in `Agent::new`, initialize:
    max_turns: 25,
```
Add the builder near `new`:
```rust
    /// Override the per-task tool-dispatch round cap (default 25).
    pub fn with_max_turns(mut self, n: usize) -> Self {
        self.max_turns = n.max(1);
        self
    }
```
In BOTH `run_task` and `run_task_with_memory`, replace `const MAX_TURNS: usize = 25;` + the `for _turn in 0..MAX_TURNS` loop bound with `self.max_turns`, and the `Err(CoreError::MaxTurnsExceeded(MAX_TURNS))` with `Err(CoreError::MaxTurnsExceeded(self.max_turns))`. (Remove the now-unused `const`.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-core` → all pass (existing tests unaffected; default 25 preserved).

- [ ] **Step 5: Gate + commit**

`cargo clippy -p entheai-core -- -D warnings` → clean. `cargo fmt -p entheai-core`.
```bash
git add crates/core/src/lib.rs
git commit -m "feat(core): configurable Agent max_turns (with_max_turns), default 25" -- crates/core/src/lib.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 4: router — strong orchestrator default + system prompt + wire config into `build_agent`

**Files:** Modify `crates/router/src/lib.rs`

Depends on Tasks 2 (`with_inference`) + 3 (`with_max_turns`). `build_agent` builds both provider and `Agent`, so wire config here.

- [ ] **Step 1: Write the failing tests** (add to router's `mod tests`; and UPDATE the existing `orchestrator_model_errors_when_nothing_set` test — it must now expect the default, not an error):

Replace `orchestrator_model_errors_when_nothing_set` with:
```rust
    #[test]
    fn orchestrator_model_defaults_to_strong_when_nothing_set() {
        let cfg = Config::from_toml_str("").unwrap();
        assert_eq!(orchestrator_model(&cfg).unwrap(), DEFAULT_ORCHESTRATOR);
        assert_eq!(orchestrator_model(&cfg).unwrap(), "deepseek/deepseek-chat");
    }
```
Add:
```rust
    #[test]
    fn orchestrator_system_prompt_default_and_override_and_append() {
        let base = Config::from_toml_str("").unwrap();
        assert_eq!(orchestrator_system_prompt(&base), DEFAULT_ORCHESTRATOR_PROMPT);

        let overridden = Config::from_toml_str("[router]\norchestrator_prompt = \"custom brain\"\n").unwrap();
        assert_eq!(orchestrator_system_prompt(&overridden), "custom brain");

        let appended = Config::from_toml_str("[router]\norchestrator_prompt_append = \"Also: prefer Rust.\"\n").unwrap();
        let p = orchestrator_system_prompt(&appended);
        assert!(p.starts_with(DEFAULT_ORCHESTRATOR_PROMPT));
        assert!(p.ends_with("Also: prefer Rust."));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-router orchestrator_model_defaults_to_strong_when_nothing_set orchestrator_system_prompt_default_and_override_and_append`
Expected: FAIL — `cannot find value DEFAULT_ORCHESTRATOR` / `no function orchestrator_system_prompt`.

- [ ] **Step 3: Implement.** At the top of `crates/router/src/lib.rs`:
```rust
/// The built-in strong orchestrator when none is configured — the current
/// strongest cheap MoE. Overridable via `[router].orchestrator` / `default_model`.
pub const DEFAULT_ORCHESTRATOR: &str = "deepseek/deepseek-chat";

/// The default orchestrator system prompt (identity + decomposition behavior).
/// Override with `[router].orchestrator_prompt`, extend with `..._append`.
pub const DEFAULT_ORCHESTRATOR_PROMPT: &str = "You are the orchestrator of entheai — a hybrid, fan-out coding agent. You are the strongest model in the swarm; your job is to plan, decompose, and synthesize, not to write code yourself.\n\nGiven a task and repository context you:\n1. Understand the goal and the provided codebase context.\n2. Decompose the work into the smallest set of independent, parallelizable sub-tasks, each matched to a role (explore, coder, test, docs, review). Prefer few well-scoped sub-tasks over many tiny ones, and only decompose when parallelism genuinely helps — a small task is a single sub-task.\n3. Give each sub-agent a precise, self-contained instruction; it sees only its own instruction, not the others'.\n4. After the sub-agents run in isolated git worktrees, synthesize their results into a coherent outcome, resolving conflicts and stating what was done.\n\nPrinciples: correctness first; minimal, focused changes; respect the repository's existing patterns; never fabricate file contents or results; if the task is ambiguous, make the most reasonable assumption and state it. Be decisive and concise.";
```
Change `orchestrator_model` to fall back to the const (delete the `.ok_or_else(...)`):
```rust
pub fn orchestrator_model(config: &Config) -> anyhow::Result<String> {
    Ok(config
        .router
        .orchestrator
        .clone()
        .or_else(|| config.default_model.clone())
        .unwrap_or_else(|| DEFAULT_ORCHESTRATOR.to_string()))
}
```
Add the prompt builder:
```rust
/// The orchestrator's system prompt: the config override or the built-in
/// default, plus an optional append.
pub fn orchestrator_system_prompt(config: &Config) -> String {
    let mut base = config
        .router
        .orchestrator_prompt
        .clone()
        .unwrap_or_else(|| DEFAULT_ORCHESTRATOR_PROMPT.to_string());
    if let Some(extra) = &config.router.orchestrator_prompt_append {
        base.push_str("\n\n");
        base.push_str(extra);
    }
    base
}
```
Wire config into `build_agent` (apply `max_turns` + inference):
```rust
    let provider = OpenAiCompatProvider::new(pcfg.base_url.clone(), api_key).with_inference(
        entheai_providers::InferenceSettings {
            request_timeout: std::time::Duration::from_secs(config.inference.request_timeout_secs),
            max_tokens: config.inference.max_tokens,
            temperature: config.inference.temperature,
            retries: config.inference.retries,
        },
    );
    Ok(Agent::new(provider, model.to_string()).with_max_turns(config.router.max_turns))
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-router` → all pass (the updated no-config test + the two new + the unchanged ones).

- [ ] **Step 5: Gate + commit**

`cargo clippy -p entheai-router -- -D warnings` → clean. `cargo fmt -p entheai-router`.
```bash
git add crates/router/src/lib.rs
git commit -m "feat(router): hardcoded-strong orchestrator default + authored system prompt + wire max_turns/inference" -- crates/router/src/lib.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 5: tools — shell timeout/cap + search cap

**Files:** Modify `crates/tools/src/shell.rs`, `crates/tools/src/search.rs`

`RunShell::new(cwd)` uses `Duration::from_secs(120)` (shell.rs:48,52) + `const MAX = 100_000` (:64). `Search::new(root)` uses `hits.len() >= 200` (search.rs:64). Add fields.

- [ ] **Step 1: Write the failing test** (add to shell.rs's tests, or create the module):

```rust
#[tokio::test]
async fn shell_honors_configured_timeout() {
    let dir = std::env::temp_dir();
    let sh = RunShell::new(&dir).with_limits(1, 100_000); // 1s timeout
    let err = sh.call(serde_json::json!({"command": "sleep 3"})).await.unwrap_err();
    assert!(matches!(err, ToolError::Timeout { secs: 1, .. }));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-tools shell_honors_configured_timeout`
Expected: FAIL — `no method named with_limits`.

- [ ] **Step 3: Implement.** In `shell.rs`, add fields + a builder + use them:
```rust
pub struct RunShell {
    cwd: PathBuf,
    timeout_secs: u64,
    output_cap: usize,
}
impl RunShell {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into(), timeout_secs: 120, output_cap: 100_000 }
    }
    /// Override the command timeout (seconds) and combined-output byte cap.
    pub fn with_limits(mut self, timeout_secs: u64, output_cap: usize) -> Self {
        self.timeout_secs = timeout_secs.max(1);
        self.output_cap = output_cap;
        self
    }
}
```
In `call`, replace `Duration::from_secs(120)` with `Duration::from_secs(self.timeout_secs)`, the two `secs: 120` in `ToolError::Timeout` with `secs: self.timeout_secs`, `const MAX: usize = 100_000;` with `let max = self.output_cap;` (rename `MAX`→`max` at both use sites), and the `"...truncated at 100000 bytes"` message with `format!("\n...[output truncated at {max} bytes]")`.

In `search.rs`, add a `max_results: usize` field (default 200) + `with_max_results(self, n)` builder, and replace the `>= 200` literal (search.rs:64) with `>= self.max_results`. Add a test `search_respects_max_results` (build a dir with >N matches, set `with_max_results(1)`, assert ≤1 hit).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-tools` → all pass.

- [ ] **Step 5: Gate + commit**

`cargo clippy -p entheai-tools -- -D warnings` → clean. `cargo fmt -p entheai-tools`.
```bash
git add crates/tools/src/shell.rs crates/tools/src/search.rs
git commit -m "feat(tools): configurable shell timeout/output-cap + search max-results" -- crates/tools/src/shell.rs crates/tools/src/search.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

**Wiring note (bin, Task 11 or here):** wherever `RunShell::new`/`Search::new` are registered (`bin/entheai/src/main.rs` `build_tools`, `crates/orchestrator` `write_registry`/`read_only_registry`), append `.with_limits(cfg.tools.shell_timeout_secs, cfg.tools.shell_output_cap)` / `.with_max_results(cfg.tools.search_max_results)`. Do that in the same commit that has `&cfg` in scope — the builders default to today's values, so unwired call sites keep working.

---

## Task 6: orchestrator + permission — fan-out policy from config

**Files:** Modify `crates/orchestrator/src/lib.rs`, `bin/entheai/src/main.rs`

`crates/orchestrator/src/lib.rs:91` builds `entheai_permission::Policy::new(true, vec![])` (hardcoded yolo) for fan-out sub-agents/coders. `Policy::new(yolo: bool, allowlist: Vec<String>)` already exists.

- [ ] **Step 1: Write the failing test.** In orchestrator's tests, add a helper-level assertion that the policy is built from config. Since the fan-out policy is constructed inline, first extract it to a testable fn:
```rust
fn fanout_policy(config: &Config) -> entheai_permission::Policy {
    entheai_permission::Policy::new(config.permission.fanout_auto_approve, config.permission.allowlist.clone())
}
```
Test:
```rust
#[test]
fn fanout_policy_follows_config() {
    let yolo = Config::from_toml_str("").unwrap(); // fanout_auto_approve defaults true
    assert!(fanout_policy(&yolo).is_yolo());
    let strict = Config::from_toml_str("[permission]\nfanout_auto_approve = false\n").unwrap();
    assert!(!fanout_policy(&strict).is_yolo());
}
```
(If `Policy` has no `is_yolo()` accessor, add a small `pub fn is_yolo(&self) -> bool { self.yolo }` to `crates/permission/src/lib.rs` in this task — it's needed for the assertion. Otherwise use the existing accessor.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-orchestrator fanout_policy_follows_config`
Expected: FAIL — `cannot find function fanout_policy` (and/or `is_yolo`).

- [ ] **Step 3: Implement.** Add `fanout_policy` (above), then replace the two inline `Policy::new(true, vec![])`/`yolo()` sites used by `run_subagent`/`run_coder`/`orchestrate_once` with `fanout_policy(config)`. (Grep `Policy::new(true` and the local `yolo()` helper in orchestrator; route them through `fanout_policy`.) Add `is_yolo` to permission if missing.

- [ ] **Step 3b: Inject the orchestrator system prompt.** The orchestrator's decompose + synthesis calls run through `orchestrate_once(config, model_id, messages)`. Prepend the authored system prompt so it takes effect on every orchestrator turn — at the top of `orchestrate_once`, before building/sending the messages:
```rust
    // Prepend the orchestrator identity prompt unless the caller already set one.
    let mut messages = messages;
    if !messages.first().map(|m| m.role == "system").unwrap_or(false) {
        messages.insert(0, ChatMessage::system(entheai_router::orchestrator_system_prompt(config)));
    }
```
(`ChatMessage` is already imported in orchestrator; `entheai_router` is already a dependency. This is the wiring that makes `[router].orchestrator_prompt`/`_append` and the authored default actually reach the model.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-orchestrator -p entheai-permission` → all pass.

- [ ] **Step 5: Single-agent policy in bin.** In `bin/entheai/src/main.rs`, change `entheai_permission::Policy::new(cli.yolo, vec![])` to `entheai_permission::Policy::new(cli.yolo || cfg.permission.yolo, cfg.permission.allowlist.clone())`.

- [ ] **Step 6: Gate + commit**

`cargo clippy -p entheai-orchestrator -p entheai-permission -p entheai -- -D warnings` → clean. `cargo fmt` those crates.
```bash
git add crates/orchestrator/src/lib.rs crates/permission/src/lib.rs bin/entheai/src/main.rs
git commit -m "feat(orchestrator): fan-out policy + single-agent yolo from config; inject orchestrator system prompt" -- crates/orchestrator/src/lib.rs crates/permission/src/lib.rs bin/entheai/src/main.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 7: memory — embedder timeout

**Files:** Modify `crates/memory/src/embed.rs`, and the `Embedder::new` call site in `bin/entheai/src/main.rs` (`build_memory`)

`embed.rs:29` has `const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30)` used in `Embedder::new`'s client builder.

- [ ] **Step 1: Write the failing test** (embed.rs tests use `wiremock`):
```rust
#[tokio::test]
async fn embedder_honors_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(2))
            .set_body_json(serde_json::json!({"data":[{"embedding":[0.1]}]})))
        .mount(&server).await;
    let emb = Embedder::new(server.uri() + "/v1", "m", 1); // 1s timeout
    let err = emb.embed("hi").await.unwrap_err();
    assert!(err.to_string().to_lowercase().contains("timeout") || err.to_string().to_lowercase().contains("timed out"), "{err}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-memory embedder_honors_timeout`
Expected: FAIL — `Embedder::new` takes 2 args, not 3.

- [ ] **Step 3: Implement.** Change `Embedder::new(base_url, model)` → `Embedder::new(base_url, model, timeout_secs: u64)`; build the client with `.timeout(Duration::from_secs(timeout_secs.max(1)))`; remove the `DEFAULT_TIMEOUT` const. Update the two existing embed tests to pass a timeout (e.g. `30`). Update the `Embedder::new(...)` call in `bin/entheai/src/main.rs` `build_memory` to pass `cfg.memory.embed_timeout_secs`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-memory` → all pass. `cargo build -p entheai` (bin call site updated).

- [ ] **Step 5: Gate + commit**

`cargo clippy -p entheai-memory -p entheai -- -D warnings` → clean. `cargo fmt` both.
```bash
git add crates/memory/src/embed.rs bin/entheai/src/main.rs
git commit -m "feat(memory): configurable embedder timeout (embed_timeout_secs)" -- crates/memory/src/embed.rs bin/entheai/src/main.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 8: tui — tick + pane caps from `[viz]`  ⚠ hot file

**Files:** Modify `crates/tui/src/lib.rs`

Re-read before editing (hot/shared file). `tui/src/lib.rs:368` `interval(Duration::from_millis(90))`; `PLAN_ROWS_CAP` (~681); `SWARM_PANE_CAP` (~694). The TUI already receives `config: entheai_config::Config` in `run`/`event_loop`.

- [ ] **Step 1: Write the failing test.** `plan_rows_for`/`swarm_rows_for` currently read the module `const`s. Change them to take the cap as a param so they're testable and config-driven:
```rust
#[test]
fn plan_rows_uses_configured_cap() {
    assert_eq!(plan_rows_for(20, 5), 5);   // 20 items clamped to cap 5
    assert_eq!(plan_rows_for(0, 8), 0);    // empty collapses
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-tui plan_rows_uses_configured_cap`
Expected: FAIL — `plan_rows_for` takes 1 arg.

- [ ] **Step 3: Implement.** Change `fn plan_rows_for(plan_len: usize) -> u16` → `fn plan_rows_for(plan_len: usize, cap: u16) -> u16` (use `cap` instead of `PLAN_ROWS_CAP`); same for `swarm_rows_for(enabled, model, cap: u16)`. Update their call sites in `event_loop` to pass `app` caps (add `viz_tick_ms`, `plan_cap`, `swarm_cap` to `App` from `config.viz`, or read `config.viz.*` where the caps are used). Replace `Duration::from_millis(90)` with `Duration::from_millis(config.viz.tick_ms.max(16))` (floor at ~60fps ceiling to avoid a 0ms busy-loop). Update the existing `swarm_pane_*` / `plan_rows` tests to pass the cap arg. Delete the now-unused `PLAN_ROWS_CAP`/`SWARM_PANE_CAP` consts (or keep them as the default values fed in — simpler: keep the consts as defaults and pass them where config isn't threaded, but prefer threading `config.viz`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-tui` → all pass (update any test calling `plan_rows_for`/`swarm_rows_for` to the new arity).

- [ ] **Step 5: Gate + commit** (explicit pathspec — hot file)

`cargo clippy -p entheai-tui -- -D warnings` → clean. `cargo fmt -p entheai-tui`.
```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): configurable tick + plan/swarm pane caps from [viz]" -- crates/tui/src/lib.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 9: companion — port + fps from `[companion]`

**Files:** Modify `crates/companion/src/render.rs`, `bin/entheai/src/main.rs`

`render.rs:8` `const FPS: f64 = 24.0` (→ `frame_interval = 1.0/FPS`). The companion is a separate binary launched by `bin` (`setup_companion`) with `--port 9876` (hardcoded at `bin/main.rs:318`, default at `companion/main.rs:39`).

- [ ] **Step 1: Implement (mechanical — no unit test needed for a launch-arg thread; a build check suffices).** In `bin/entheai/src/main.rs` `setup_companion`, replace the hardcoded `"9876"` port arg with `cfg.companion.port.to_string()`, and pass an `--fps` arg = `cfg.companion.fps.to_string()`. In `crates/companion/src/main.rs`, add an `--fps: f64` clap arg (default 24.0) and thread it into the renderer; in `crates/companion/src/render.rs` replace `const FPS` usage with the passed-in fps (or keep `FPS` as the default value the arg falls back to). Ensure `setup_companion` has `cfg` in scope (it takes `&Config`).

- [ ] **Step 2: Verify**

Run: `cargo build -p entheai-companion -p entheai` → compiles. `cargo test -p entheai-companion` → existing tests pass.

- [ ] **Step 3: Gate + commit**

`cargo clippy -p entheai-companion -p entheai -- -D warnings` → clean. `cargo fmt` both.
```bash
git add crates/companion/src/render.rs crates/companion/src/main.rs bin/entheai/src/main.rs
git commit -m "feat(companion): configurable port + fps from [companion]" -- crates/companion/src/render.rs crates/companion/src/main.rs bin/entheai/src/main.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 10: radio — download timeout

**Files:** Modify `crates/radio/src/lib.rs`, and its construction site (`bin`/`tui` — wherever the radio is built)

`radio/src/lib.rs:273` `const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300)`.

- [ ] **Step 1: Implement.** Grep how the radio is constructed (`Radio::new`/`spawn`). Add a `download_timeout_secs: u64` param (or a field with a `with_download_timeout` builder defaulting to 300), replace the `DOWNLOAD_TIMEOUT` use with it, and pass `cfg.radio.download_timeout_secs` from the construction site. If a unit test is impractical (it shells out to yt-dlp), a build check + a param-plumbing assertion (construct with a custom value, read it back via a getter) suffices.

- [ ] **Step 2: Verify + commit**

Run: `cargo build -p entheai-radio` (+ whatever builds the radio) → compiles; `cargo test -p entheai-radio` → passes. `cargo clippy -p entheai-radio -- -D warnings` → clean. `cargo fmt -p entheai-radio`.
```bash
git add crates/radio/src/lib.rs <construction-site-file>
git commit -m "feat(radio): configurable download timeout" -- crates/radio/src/lib.rs <construction-site-file>
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 11: bin — MCP spawn timeout + Sentry DSN + orchestrator single-agent fallback  ⚠ hot file

**Files:** Modify `bin/entheai/src/main.rs`

Re-read before editing. `bin/main.rs:255,272` MCP spawn timeout `Duration::from_secs(10)`; `:126` hardcoded Sentry DSN fallback; `:50` single-agent model resolution errors when unset.

- [ ] **Step 1: Implement (three edits).**
  1. **MCP spawn timeout:** replace `Duration::from_secs(10)` (both sites in the MCP spawn/`load_tools` timeout) with `Duration::from_secs(cfg.mcp_defaults.spawn_timeout_secs)`. `build_tools` takes `&Config` — confirm `cfg` is in scope (it is).
  2. **Sentry DSN:** in `init_telemetry`, resolve DSN as `config.telemetry.sentry_dsn` → `SENTRY_DSN` env → the existing hardcoded fallback string. NOTE: `init_telemetry` currently runs before the config is parsed — reorder so config is read first, or pass the DSN in. Change `init_telemetry()` to `init_telemetry(dsn: Option<String>)` and call it after `Config::from_toml_str`, passing `cfg.telemetry.sentry_dsn.clone().or_else(|| std::env::var("SENTRY_DSN").ok())` and OR-ing the hardcoded fallback inside.
  3. **Single-agent orchestrator fallback:** at `:50`, change `--model → default_model → error` to `--model → default_model → entheai_router::DEFAULT_ORCHESTRATOR` (so a bare config runs). Use `.unwrap_or_else(|| entheai_router::DEFAULT_ORCHESTRATOR.to_string())` instead of the `.context("no model…")?`.
  Also fold in the Task 5 wiring note if not already done: `RunShell::new(...).with_limits(cfg.tools.shell_timeout_secs, cfg.tools.shell_output_cap)` and `Search::new(...).with_max_results(cfg.tools.search_max_results)` in `build_tools`.

- [ ] **Step 2: Verify**

Run: `cargo build -p entheai` → compiles. Offline smoke: a config with only a provider block (no `default_model`) should resolve the model to `deepseek/deepseek-chat` and not error at startup (it'll fail later only if the provider/key is missing) — `cargo run -q -p entheai -- --config <bare.toml> --no-companion "hi" 2>&1 | tail -3` shows a provider/auth error, NOT a "no model" error.

- [ ] **Step 3: Gate + commit** (explicit pathspec — hot file)

`cargo clippy -p entheai -- -D warnings` → clean. `cargo fmt -p entheai`.
```bash
git add bin/entheai/src/main.rs
git commit -m "feat(bin): MCP spawn timeout + Sentry DSN from config + strong orchestrator fallback + tool limits" -- bin/entheai/src/main.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Final verification

- [ ] `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings` → clean.
- [ ] `cargo test --workspace` → all green.
- [ ] **Bare-config smoke:** an `entheai.toml` with only `[providers.deepseek]` (+ its `api_key_env`) resolves the orchestrator to `deepseek/deepseek-chat` and injects the authored system prompt — no "no orchestrator/model" error.
- [ ] **Override smoke:** setting `[router].max_turns`, `[inference].request_timeout_secs`, `[tools].shell_timeout_secs`, `[permission].fanout_auto_approve`, `[viz].tick_ms` each observably changes behavior; omitting them reproduces today's behavior.
- [ ] Confirm no LOW-tier value was exposed (scope discipline): branch templates, git identity, spinner glyphs/verbs, codename lists, layout row constants, socket path, worktree location, verify shell all remain hardcoded.

## Notes for the executor

- **Non-breaking builders** (`with_inference`, `with_max_turns`, `with_limits`, `with_max_results`) keep each low-level crate green on its own; `router::build_agent` + `bin` are the wiring points. Order: config (1) → providers/core builders (2,3) → router wiring (4) → the rest.
- **Hot files** (`crates/config`, `crates/tui`, `bin/entheai/src/main.rs`) — always re-read before editing, scoped explicit-pathspec commit, push immediately, rebase on non-FF.
- Every field defaults to today's value, so a partially-applied plan never regresses behavior.
- `DEFAULT_ORCHESTRATOR = "deepseek/deepseek-chat"` needs `[providers.deepseek]`; that's a config concern, not a code error — `build_agent` already reports a missing provider clearly.
