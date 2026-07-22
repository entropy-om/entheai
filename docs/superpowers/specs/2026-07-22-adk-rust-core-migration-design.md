# entheai core agent loop → adk-rust — Design

## 1. Purpose & role

Replace entheai's hand-rolled agent loop (`crates/core::Agent<P: Provider>::run_task` /
`run_task_with_memory`) and its provider abstraction (`crates/providers`) with
[`adk-rust`](https://github.com/zavora-ai/adk-rust) (`adk-agent` + `adk-model` +
`adk-runner` + `adk-tool`), as the first front of a larger, multi-phase move to
adopt adk-rust as entheai's agent framework. This is a big-bang, single-PR
cutover with no runtime fallback: once merged, `crates/providers` and the old
`run_task` are gone.

This spec covers front #1 only (core agent loop + provider abstraction, since
they're the same generic parameter on `Agent<P>` and can't be separated). The
other fronts identified during brainstorming — tool system (`crates/tools`
kept, only wrapped), MCP integration (`crates/mcp`, untouched for now),
orchestrator fanout beyond the shared `run_task` call, and the
memory/ultra-nodes worker pool that motivated this whole investigation — are
separate follow-up fronts, not in scope here.

## 2. Why (context)

Session context: building a local-LLM worker pool ("ultra-nodes") for
memory-pp's retrieval/compression, the user asked to build it on
`adk-rust` rather than entheai's own `Provider`/tool stack. Investigation
showed adk-rust could be contained to just that corner, but the user chose
full adoption across entheai instead — starting with the highest-risk,
highest-value front: the core agent loop itself, since every other adk-rust
integration (including the original ultra-nodes ask) will eventually sit on
top of whatever agent/tool/provider stack is canonical.

## 3. Current state (what's being replaced)

- `crates/core/src/lib.rs`: `Agent<P: Provider>` struct; `run_task` (agentic
  tool-dispatch loop, hard-capped at `max_turns`, default 25); `run_task_with_memory`
  (same loop + pre-task retrieval injection via `MemoryRuntime`/`PromptProcessor`
  + post-task trajectory recording); `AgentEvent` enum (`Thinking`, `ToolStarted`,
  `ToolFinished`, `Token`) streamed over an `UnboundedSender<AgentEvent>` that
  `crates/tui` renders live.
- `crates/providers/src/lib.rs`: `Provider` trait (`stream_chat`, `complete`,
  `stream_complete`), `ChatMessage`, `StreamEvent`, an OpenAI-compatible HTTP
  client implementation. Every provider entheai supports today (OpenAI,
  Anthropic, osaurus, any other OpenAI-compatible endpoint) goes through this
  one client via config-driven `base_url`/`api_key_env`.
- Permission gating: `crates/permission::{Policy, Prompter}`, checked inline
  inside `run_task`'s per-tool-call `dispatch_call` before the tool actually
  runs. A denial becomes a `"error: permission denied"`-style string fed back
  to the model as a normal tool result — never a hard `Err` that aborts the run.
- Callers of `Agent<P>::run_task`: `crates/tui` (interactive loop),
  `crates/orchestrator` (each fan-out coder is also an `Agent<P>` run),
  `bin/entheai` (one-shot CLI path).

## 4. Architecture

- `crates/core` is gutted and rebuilt as a thin wrapper crate (`EntheaiAgent`,
  name TBD at plan time) around `adk_agent::LlmAgent` + `adk_runner::Runner`.
  Callers keep calling into `crates/core` — they don't talk to adk-rust types
  directly — but the wrapper's job shrinks from "implement the loop" to
  "configure adk-agent + wire callbacks + translate at the boundary."
- `crates/providers` is deleted. Model access goes through `adk-model`:
  Gemini/OpenAI/Anthropic natively; osaurus (and any other OpenAI-compatible
  local/remote endpoint) via the same OpenAI-compatible-preset mechanism
  adk-rust already uses for Fireworks/Together/Mistral/Perplexity/etc.
  (custom `base_url`, no API key).
- `crates/tools`'s `Tool` trait and existing tool implementations (shell,
  file edit, etc.) are kept unchanged. A new adapter module wraps each
  entheai `Tool` as an `adk_tool::Tool` so adk-agent's loop can call them
  without entheai's tools being rewritten.
- Permission gating moves from inline-in-`dispatch_call` to an adk-core
  `before_tool` callback (adk-rust's own "before-tool callbacks" /
  human-in-the-loop confirmation feature). `Policy`/`Prompter` themselves are
  unchanged — only where they're invoked from changes.
- `AgentEvent` is deleted. `crates/tui`, `crates/companion`, and the
  orchestrator's coder-output capture consume adk-runner's native event
  stream directly — no entheai-side translation enum.
- Memory (`entheai_memory`/`entheai_memory_pp`) pre-task retrieval injection
  and post-task trajectory recording stay entheai-native code (unchanged
  internals), triggered from adk-core's `before_agent`/`after_agent` callback
  hooks instead of being inlined in the hand-rolled loop.
- Config: the existing `[providers.*]` TOML shape and `"provider/model"`
  string convention (`default_model`, `AgentConfig.model`) are kept —
  adk-rust's own examples use the same `provider/model` addressing, so the
  config format doesn't need to change, only what resolves it.
- Workspace `rust-version` (currently `1.80`, `Cargo.toml:10`) rises to
  whatever adk-rust's actual MSRV is (README states 1.94+ / edition 2024 at
  time of writing — confirm exact floor at plan time against the version
  actually pinned). `cargo-dist`'s build toolchain config updates to match.

## 5. Data flow

- **Inbound history**: entheai's `Vec<ChatMessage>` is converted at the
  wrapper boundary into adk-core's `Content`/`Part` representation. Whether
  adk-agent wants a raw sequence per call or an `adk_session::Session` object
  seeded once per task is an open question (§8) — adk-core's exact session
  model needs a close read during implementation planning, not resolved here.
- **Tool dispatch**: adk-agent decides to call a tool → invokes the wrapped
  `adk_tool::Tool::call` → delegates to entheai's unchanged
  `ToolRegistry::dispatch` → gated first by the `before_tool` callback. A
  denial returns a tool-result `Part` carrying the denial text (same
  non-fatal convention as today), never an `Err` that aborts the run.
- **Streaming**: adk-runner's event stream (tokens, tool-start/tool-end,
  etc.) is forwarded directly to whatever sink `crates/tui` or `bin/entheai`'s
  one-shot path wants. They subscribe to adk's stream type; no intermediate
  entheai enum.
- **Memory**: `before_agent` callback replicates today's "insert retrieval
  brief immediately before the last user message" logic, using the same
  `MemoryScope`/`PromptProcessor`/`MemoryRuntime` types unchanged.
  `after_agent` callback replicates today's trajectory/`ToolEvidence`
  recording, same types unchanged.

## 6. Error handling

- `CoreError` is retired. Errors surface as `adk_core::Error`/`AdkError` up
  through the wrapper. Callers (bin/entheai's top-level error reporting,
  the TUI's error surface) adapt to the new error type.
- Permission denial stays non-fatal by construction (§5) — this is a hard
  requirement carried over from today's behavior, not a nice-to-have.
- `CoreError::MaxTurnsExceeded`'s cap (default 25, guards against a looping
  model burning unbounded paid API calls, critical under `--yolo` where no
  human approves each call) has no confirmed adk-agent equivalent. Plan-time
  discovery: check adk-rust's `LoopAgent`/"Loop termination" tool, or wrap the
  runner in entheai's own turn-counting timeout if nothing built-in fits.
  Either way, the cap must survive this migration — dropping it silently
  would remove a real cost/safety guardrail.

## 7. Testing

- Every existing `crates/core` behavioral test is ported as a parity test
  against the new wrapper, same assertions:
  `run_task_dispatches_tool_then_returns_final_answer`,
  `run_task_caps_runaway_tool_loops`,
  `run_task_emits_thinking_and_tool_events`,
  `run_task_feeds_back_permission_denied_tool_result`,
  `run_task_feeds_back_unknown_tool_error`,
  `run_task_feeds_back_bad_json_args_error`.
  These prove behavior parity, not merely "it compiles."
- `adk-eval` (adk-rust's own trajectory-validation/LLM-judged scoring crate)
  is evaluated during planning for coverage entheai's own test harness
  doesn't have an equivalent for; not a hard requirement of this spec.
- Full workspace `cargo test`/`cargo clippy -- -D warnings` gate before
  merge, matching established practice in this codebase.
- No fallback engine exists after this lands (§1) — the PR cannot merge
  until 100% of the ported parity tests pass. This is the safety net in
  place of the flag-gated rollout the user declined.

## 8. Scope, non-goals, open questions

**In scope:** `crates/core`'s agent loop, `crates/providers`, the callers
that directly invoke `Agent<P>::run_task`/`run_task_with_memory`
(`crates/tui`, `crates/orchestrator`, `bin/entheai`) to the extent needed to
keep them compiling and behaviorally correct against the new wrapper.

**Non-goals (separate fronts, not touched here):**
- `crates/tools`/`crates/mcp` internals beyond the thin `adk_tool::Tool`
  adapter — no rewrite of tool implementations or the MCP client.
- `crates/orchestrator`'s `WorkerPool`/`AgyExecutor` fan-out mechanics beyond
  what's forced by the shared `run_task` call — the pool itself is untouched.
- The memory/ultra-nodes local-LLM worker pool that originally motivated
  this investigation (brain-radio theming, knowledge-worker ingestion,
  osaurus-backed reranking/compression) — a separate spec, to be revisited
  once this front lands.
- Any adk-rust feature not needed for this migration (RAG, payments,
  realtime voice, browser automation, AWP, sandbox) — not adopted just
  because the dependency is now present.

**Open questions for the implementation plan to resolve by reading adk-core
directly (not guessed here):**
- Exact `Content`/`Part`/`Event` type shapes and the session-vs-raw-history
  question (§5).
- Exact adk-rust MSRV to pin (README says 1.94+/edition 2024 — verify against
  the version actually vendored).
- Whether `MaxTurnsExceeded`'s guardrail has a built-in adk-agent equivalent
  (§6).
- Whether adk-model's OpenAI-compatible preset needs any entheai-side glue to
  address osaurus specifically, or whether config alone suffices (mirrors
  how Fireworks/Together/etc. are already handled by adk-rust).

## 9. Success criteria

- `crates/providers` deleted from the workspace; no crate depends on it.
- `crates/core` contains no hand-rolled tool-dispatch loop — the loop lives
  in `adk-agent`.
- All six ported parity tests (§7) pass unchanged in intent (same scenario,
  same assertion), proving no behavioral regression in tool-calling,
  streaming, permission-gating, or turn-capping.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D
  warnings` both pass clean.
- The TUI's live event rendering (thinking indicator, tool-start/tool-finish,
  token streaming) works unchanged from a user's perspective, even though
  the underlying event type changed.
- osaurus continues to work as a model backend with no config format change
  visible to the user.
