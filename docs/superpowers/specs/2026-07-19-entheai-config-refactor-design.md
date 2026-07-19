# entheai Config Refactor — Design

**Date:** 2026-07-19 · **Status:** approved design, pre-plan
**Scope:** lift most currently-hardcoded behavior/cost/safety knobs into `entheai.toml` (via `crates/config`) with sane defaults, while the **orchestrator** gains a hardcoded *strong* default model (`deepseek/deepseek-chat` = DeepSeek V4 Pro) plus a first authored **orchestrator system prompt**. Backward-compatible: every new field defaults, so existing configs keep working.

## 1. Purpose

Today many behavior/cost/safety values are hardcoded `const`s scattered across crates (an audit found ~40; ~15 are HIGH/MED value). Users can't tune the agent's turn cap, tool timeouts, sampling, or the fan-out permission policy — and the providers HTTP client has **no request timeout at all**. This refactor:

1. Makes the **HIGH + curated MED** knobs configurable (skip LOW cosmetic/structural ones — YAGNI).
2. Gives the **orchestrator** a hardcoded *strong* default so it's never weak or erroring out of the box, while staying overridable.
3. Ships the **first authored orchestrator system prompt** (identity + decomposition behavior), configurable via override/append.

## 2. Orchestrator — the special case

**Model resolution.** Add `const DEFAULT_ORCHESTRATOR: &str = "deepseek/deepseek-chat"` in `crates/router`. `orchestrator_model(config)` becomes: `config.router.orchestrator` → `config.default_model` → **`DEFAULT_ORCHESTRATOR`** (replacing the current hard error at `router/src/lib.rs:12-14`). The one-shot / TUI single-agent path in `bin/entheai/src/main.rs` (`--model` → `default_model` → error) gets the same `DEFAULT_ORCHESTRATOR` fallback, so entheai runs with only a provider key configured. **Still overridable** via `[router].orchestrator` (fan-out) / `--model` / `default_model` (single-agent) — the default is strong, not locked.

*Provider note:* the default id `deepseek/deepseek-chat` needs `[providers.deepseek]` configured (base_url + `api_key_env`). If the referenced provider is missing, `build_agent` already errors with a clear message — unchanged behavior, just a better default target.

**Orchestrator system prompt.** A new authored default injected as the orchestrator's system message on its decompose + synthesis calls (the existing terse decompose *format* instructions remain as the task message underneath). Configurable:
- `[router].orchestrator_prompt` (`Option<String>`) — **replace** the default identity prompt.
- `[router].orchestrator_prompt_append` (`Option<String>`) — **append** to the default (e.g. house style, repo conventions).

The default (`DEFAULT_ORCHESTRATOR_PROMPT`, a `const` in `crates/router`, exposed alongside `orchestrator_system_prompt(config) -> String`):

> You are the orchestrator of **entheai** — a hybrid, fan-out coding agent. You are the strongest model in the swarm; your job is to **plan, decompose, and synthesize**, not to write code yourself.
>
> Given a task and repository context you:
> 1. Understand the goal and the provided codebase context.
> 2. Decompose the work into the **smallest set of independent, parallelizable** sub-tasks, each matched to a role (`explore`, `coder`, `test`, `docs`, `review`). Prefer few well-scoped sub-tasks over many tiny ones, and only decompose when parallelism genuinely helps — a small task is a single sub-task.
> 3. Give each sub-agent a **precise, self-contained** instruction; it sees only its own instruction, not the others'.
> 4. After the sub-agents run in isolated git worktrees, **synthesize** their results into a coherent outcome, resolving conflicts and stating what was done.
>
> Principles: correctness first · minimal, focused changes · respect the repository's existing patterns · never fabricate file contents or results · if the task is ambiguous, make the most reasonable assumption and state it. Be decisive and concise.

## 3. Config schema — new & extended sections

All fields have `#[serde(default = "…")]` defaults (mirroring the existing `MemoryConfig`/`VizConfig` pattern), so omitting them yields today's behavior.

| Section | Field (default) | Replaces (audit ref) |
|---|---|---|
| `[router]` *(extend)* | `max_turns: usize` (25) | `MAX_TURNS` — `core/src/lib.rs:115,178` |
| | `orchestrator_prompt: Option<String>` (none) | new |
| | `orchestrator_prompt_append: Option<String>` (none) | new |
| `[inference]` *(new)* | `request_timeout_secs: u64` (120) | providers client — no timeout today (`providers/src/lib.rs:151`) |
| | `max_tokens: Option<u32>` (none) | request body never sends it (`:191-259`) |
| | `temperature: Option<f32>` (none) | ditto |
| | `retries: u32` (2) | no retry today |
| `[tools]` *(new)* | `shell_timeout_secs: u64` (120) | `shell.rs:48,52` |
| | `shell_output_cap: usize` (100000) | `shell.rs:64,71` |
| | `search_max_results: usize` (200) | `search.rs:64,78` |
| `[permission]` *(new)* | `yolo: bool` (false) | new (single-agent default) |
| | `allowlist: Vec<String>` ([]) | new |
| | `fanout_auto_approve: bool` (true) | hardcoded `Policy::new(true, vec![])` — `orchestrator/src/lib.rs:91` |
| `[mcp_defaults]` *(new)* | `spawn_timeout_secs: u64` (10) | MCP spawn timeout — `bin/main.rs:255,272` |
| `[memory]` *(extend)* | `embed_timeout_secs: u64` (30) | `embed.rs:29` `DEFAULT_TIMEOUT` |
| `[viz]` *(extend)* | `tick_ms: u64` (90) | TUI tick — `tui/src/lib.rs:368` |
| | `plan_rows_cap: u16` (8) | `PLAN_ROWS_CAP` — `:681` |
| | `swarm_rows_cap: u16` (8) | `SWARM_PANE_CAP` — `:694` |
| `[companion]` *(extend)* | `port: u16` (9876) | `companion/main.rs:39`, `bin/main.rs:318` |
| | `fps: f64` (24.0) | `companion/render.rs:8` `FPS` |
| `[radio]` *(new)* | `download_timeout_secs: u64` (300) | `radio/src/lib.rs:273` `DOWNLOAD_TIMEOUT` |
| `[telemetry]` *(new)* | `sentry_dsn: Option<String>` (none) | hardcoded DSN fallback — `bin/main.rs:126` |

**Sampling scope:** `[inference]` `max_tokens`/`temperature` are **global** (applied to every provider request). Per-`[agents.<role>]` sampling overrides are a deliberate non-goal for v1 (YAGNI). **Telemetry:** `[telemetry].sentry_dsn` is consulted first, then the `SENTRY_DSN` env var, then the existing hardcoded fallback (kept so crash reporting still works out of the box).

**`[mcp_defaults]` naming:** `[mcp]` is a `HashMap<name, McpServerConfig>` (per-server), so global MCP settings live in a sibling `[mcp_defaults]` section rather than colliding with the server map.

## 4. Architecture & consumer wiring

`crates/config` is the mechanical core: add the structs + `#[serde(default)]` fns + `impl Default` + tests, extending the established pattern. No consumer logic changes there.

Each consumer then **replaces its `const`/literal with the threaded config value**, one independent task per crate:

- **`crates/router`** — `DEFAULT_ORCHESTRATOR` const + fallback in `orchestrator_model`; build the orchestrator system prompt (default ⊕ `orchestrator_prompt`/`_append`) and expose it (e.g. `orchestrator_system_prompt(config) -> String`).
- **`crates/core`** — `run_task`/`run_task_with_memory` take `max_turns` from config (thread it in; both call sites use the same value).
- **`crates/providers`** — the client builder applies `request_timeout_secs`; requests include `max_tokens`/`temperature` when set; a small retry wrapper (`retries`, exponential backoff) around the completion call. **This is the highest-value change** (no timeout today = a hung provider stalls a turn forever).
- **`crates/tools`** — `RunShell` reads `shell_timeout_secs`/`shell_output_cap`; `Search` reads `search_max_results`.
- **`crates/orchestrator` + `crates/permission`** — fan-out coder/sub-agent policy is built from `[permission] fanout_auto_approve`/`allowlist` instead of the hardcoded `Policy::new(true, vec![])`. The **single-agent** path in `bin` builds `Policy::new(cli.yolo || config.permission.yolo, config.permission.allowlist)` — the `--yolo` CLI flag still forces yolo on, `[permission].yolo` is the config default when the flag is absent.
- **`crates/memory`** — `Embedder` timeout from `embed_timeout_secs`.
- **`crates/tui`** — tick interval, `plan_rows_cap`, `swarm_rows_cap` from `[viz]`.
- **`crates/companion` + `bin`** — port + fps threaded to the companion launch.
- **`crates/radio`** — download timeout from config.
- **`bin/entheai`** — MCP spawn timeout from `[mcp_defaults]`; Sentry DSN from `[telemetry]` then env then fallback; the `DEFAULT_ORCHESTRATOR` single-agent fallback.

Consumers that don't already receive `&Config` get the specific value(s) passed at construction (e.g. `RunShell::new(root, timeout, cap)`), keeping crates decoupled from the whole config type where practical.

## 5. Testing

- **config:** a defaults test + a TOML round-trip test per new/extended section (assert defaults, assert overrides parse) — extend the existing `#[cfg(test)]` block.
- **router:** `orchestrator_model` returns `DEFAULT_ORCHESTRATOR` when both `router.orchestrator` and `default_model` are unset; an explicit `[router].orchestrator` still wins. `orchestrator_system_prompt` = default when unset, replaced by `orchestrator_prompt`, and `default + append` when `orchestrator_prompt_append` is set.
- **core:** `max_turns = 1` stops after one tool-dispatch round (assert via the recording provider harness).
- **providers:** a request-timeout test (a slow mock server → the client errors within the configured timeout); `max_tokens`/`temperature` appear in the request body when set; retry attempts on a transient 5xx.
- **tools:** `shell_timeout_secs` kills a long command; `search_max_results` caps hits; `shell_output_cap` truncates.
- **orchestrator/permission:** `fanout_auto_approve = false` makes a gated fan-out tool call NOT auto-approve.
- **memory/tui/companion/radio:** each new value is read (a targeted unit test where cheap; otherwise a construction test).

## 6. Success criteria

- `orchestrator_model` never errors on a bare config: with only a provider key, entheai runs on `deepseek/deepseek-chat`.
- Setting `[router].orchestrator` / `--model` still overrides — the strong default is a fallback, not a lock.
- The orchestrator injects the authored system prompt on start; `orchestrator_prompt`/`_append` change it.
- Every HIGH/MED knob in §3 is settable in `entheai.toml` and observably changes behavior; omitting it reproduces today's behavior.
- The providers client has a real request timeout and optional sampling + retry (previously absent).

## 7. Non-goals (v1)

Per-role sampling overrides · exposing the LOW tier (branch-name templates, worktree location, coder git identity, verify shell, spinner glyphs/verbs, codename word lists, layout row constants, truncation widths, socket path) · a config migration/versioning system · hot-reload of config · a `config validate`/`config print` subcommand (nice, but separate).
