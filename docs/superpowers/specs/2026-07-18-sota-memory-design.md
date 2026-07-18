# SOTA memory for entheai - design

Status: approved
Date: 2026-07-18

## Context

entheai already has an `entheai-memory` crate with a typed `Memory` trait,
five namespaces, SQLite persistence, optional OpenAI-compatible embeddings, and
flat cosine search. Its focused crate tests pass locally. The missing piece is
that memory is not yet active in the agent loop: `Agent::run_task` calls the
provider, dispatches tools, and returns the final answer without retrieving
prior knowledge, spilling large evidence, or recording trajectories.

This spec turns the existing memory substrate into a SOTA-shaped runtime for
coding-agent work. The first implementation must be shippable and testable
inside the current Rust workspace, while leaving clear seams for HNSW/ANN,
codebase-memory MCP federation, temporal graph recall, and agentic
consolidation.

## Research basis

The design borrows from current agent-memory systems, but maps them to
entheai's local-first Rust architecture:

- MemGPT: hierarchical context management, where useful memories are paged into
  the model context rather than stuffing all history into every call.
  https://arxiv.org/abs/2310.08560
- Mem0: selective extraction, consolidation, and retrieval of salient facts,
  with graph memory as an accuracy upgrade over simple vector chunks.
  https://arxiv.org/abs/2504.19413
- A-Mem: Zettelkasten-style linked notes that evolve as new memories arrive.
  https://arxiv.org/abs/2502.12110
- Zep/Graphiti: temporal knowledge graph memory with provenance and fact
  validity windows for dynamic agent state.
  https://arxiv.org/abs/2501.13956
- HippoRAG 2: graph-assisted retrieval for factual, sense-making, and
  associative memory tasks.
  https://arxiv.org/abs/2502.14802

The resulting principle is: vector search is a useful substrate, not the whole
memory system. entheai memory should retrieve, compress, link, score, and age
evidence according to the task.

## Goals

- Make memory active in one-shot CLI, TUI, and fan-out paths.
- Preserve the internal five-namespace model: `codebase`, `learnings`,
  `trajectories`, `tools`, `subagents`.
- Inject bounded, source-labeled memory context before model calls.
- Spill large tool outputs to `tools` memory and feed the model compact
  pointers plus previews.
- Store durable post-task residue into `learnings` and structured execution
  records into `trajectories`.
- Let fan-out sub-agents write and read scoped scratch/results through
  `subagents`.
- Keep runtime memory failures non-fatal to the agent loop unless the caller
  explicitly asks for strict memory behavior.
- Add configuration for memory enablement, storage path, embedding provider,
  retrieval budgets, spill thresholds, and future backend selection.
- Update public docs so the namespace story matches the crate and internal
  architecture spec.

## Non-goals

- No full temporal graph engine in the first implementation.
- No HNSW dependency in the first implementation unless flat search becomes a
  measured bottleneck during implementation.
- No remote/cloud memory service.
- No personal Honcho-style user model in this pass. `learnings` may store user
  preferences, but a dedicated personalization engine remains separate.
- No generative summarizer dependency. Extraction starts deterministic and
  prompt-driven through existing providers only where needed.
- No change to the provider wire format.

## Namespace model

The authoritative namespace set is the one already implemented in
`crates/memory` and described by the internal hybrid-agent spec:

| Namespace | Tier | Owns |
|---|---|---|
| `codebase` | long-term | Repository graph, symbols, files, architecture notes, ADRs. |
| `learnings` | long-term | Durable facts, preferences, conventions, and "how we solved X". |
| `trajectories` | long-term | Reasoning paths, tool choices, outcomes, scores, regressions. |
| `tools` | working | Large tool outputs, evidence blobs, success/failure signals. |
| `subagents` | working | Per-sub-agent scratch, outputs, and orchestration handoff state. |

The public docs currently describe `session`, `project`, `user`, and `skills`.
That is outdated for the crate now in the repo. The docs should be updated to
explain that session/project/user/skills are logical sources or future derived
views, while the physical runtime namespaces are the five above.

## Architecture

```
CLI/TUI/orchestrator
        |
        v
Agent::run_task
        |
        v
MemoryRuntime
  - retrieve_before()
  - enrich_messages()
  - record_tool_result()
  - record_final_answer()
  - record_failure()
        |
        +--> MemoryStore adapter
        |      - current: SqliteStore + flat cosine
        |      - future: HNSW/ANN adapter
        |
        +--> CodebaseMemory adapter
        |      - current: local codebase namespace fallback
        |      - future: codebase-memory-mcp federation
        |
        +--> Consolidator
        |      - current: deterministic extraction and caps
        |      - future: linked-note evolution, temporal graph facts
        |
        +--> TrajectoryRecorder
               - current: structured JSON entries in trajectories
               - future: router/model learning and dogfeed export
```

`MemoryRuntime` is the hot-path integration boundary and belongs in
`entheai-memory` as a new runtime module. It may depend on
`entheai-providers::ChatMessage` for memory-context construction, but it must
not depend on `entheai-core`. `entheai-core` may depend on `entheai-memory` and
call the runtime from the agent loop. `core` must not learn SQLite details,
embedding details, or future graph details.

## Core interfaces

The implementation should introduce these public types in `entheai-memory`:

```rust
pub struct MemoryRuntime {
    memory: entheai_memory::SharedMemory,
    config: MemoryRuntimeConfig,
}

pub struct MemoryRuntimeConfig {
    pub enabled: bool,
    pub strict: bool,
    pub retrieve_codebase: usize,
    pub retrieve_learnings: usize,
    pub retrieve_trajectories: usize,
    pub max_context_chars: usize,
    pub tool_spill_chars: usize,
    pub evidence_tools: Vec<String>,
}

pub struct MemoryScope {
    pub session_id: String,
    pub task_id: String,
    pub cwd: std::path::PathBuf,
    pub role: Option<String>,
}

pub struct ToolEvidence {
    pub call_id: String,
    pub name: String,
    pub args: String,
    pub result: String,
    pub allowed: bool,
}
```

`Agent::run_task` should gain a memory-aware entry point rather than changing
every caller to build memory prompts manually:

```rust
pub async fn run_task_with_memory(
    &self,
    messages: Vec<ChatMessage>,
    registry: &entheai_tools::ToolRegistry,
    policy: &entheai_permission::Policy,
    prompter: &mut impl entheai_permission::Prompter,
    events: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    memory: Option<&MemoryRuntime>,
    scope: MemoryScope,
) -> Result<String, CoreError>
```

Existing `run_task` remains as the memory-free compatibility path and delegates
to the same internal loop with `memory = None`.

## Pre-task retrieval

Before the first provider call for a task, `MemoryRuntime` builds a retrieval
query from:

- the latest user message
- the current working directory
- optional role (`explore`, `coder`, `reviewer`, `test`, `docs`)
- a compact view of already-loaded messages

It searches:

- `codebase`: repo-specific context and future MCP graph facts
- `learnings`: durable conventions and prior solutions
- `trajectories`: similar tasks and outcomes

It then emits a single bounded `system` message placed after any caller-supplied
system messages and before user/task messages:

```text
Memory context:

[codebase score=0.82 key=...]
...

[learnings score=0.77 key=...]
...

[trajectories score=0.71 key=...]
...
```

Rules:

- Include source namespace, key, and score for every item.
- Respect `max_context_chars` after formatting.
- Prefer higher score, newer `updated_at`, and exact cwd/session metadata
  matches when trimming.
- If memory search fails and `strict = false`, emit no memory block and continue.
- If `strict = true`, return a `CoreError` variant that preserves the memory
  error text.

## Tool spillover

After every tool call, `MemoryRuntime::record_tool_result` decides whether to
store the output in `tools`.

Store when:

- result length exceeds `tool_spill_chars`
- result starts with `error:`
- tool name is listed in `evidence_tools`
- caller marks the result as evidence-bearing

When an output is spilled, the model receives a compact replacement:

```text
tool result stored in memory://tools/<key>
preview:
<first N chars>
```

The full result remains retrievable by key. This prevents large command output
from consuming context while preserving evidence for later verification.

## Post-task extraction

When the model returns a final answer, `MemoryRuntime::record_final_answer`
captures a structured trajectory:

```json
{
  "schema": "entheai.trajectory.v1",
  "session_id": "...",
  "task_id": "...",
  "cwd": "...",
  "role": null,
  "started_at": 1784390000000,
  "finished_at": 1784390012345,
  "model": "provider/model",
  "tool_calls": [
    {"name": "read_file", "allowed": true, "stored_key": "tools/..."}
  ],
  "outcome": "answered",
  "final_answer_preview": "..."
}
```

It also extracts a small set of durable `learnings` candidates. Initial
extraction is deterministic:

- explicit user preferences from user messages
- project conventions discovered in AGENTS/spec/docs
- commands that succeeded or failed and why
- architectural decisions made by this task
- repeated tool failures or permission patterns

Each learning entry must include metadata:

```json
{
  "schema": "entheai.learning.v1",
  "source": "post_task_extraction",
  "session_id": "...",
  "task_id": "...",
  "cwd": "...",
  "confidence": 0.6,
  "tags": ["rust", "memory", "tooling"]
}
```

Later agentic consolidation may merge, link, decay, or supersede these entries.
The first implementation should avoid overwriting unrelated learnings.

## Fan-out behavior

Fan-out uses the same runtime with per-role scopes:

- Decomposition retrieves from `trajectories` to reuse successful task shapes.
- Each sub-agent writes result summaries and high-value evidence to
  `subagents`.
- The synthesizer retrieves `subagents` entries by `session_id` and `task_id`
  before composing the final answer.
- Sub-agent memory keys include role and a stable task index so parallel writes
  do not collide.

The first implementation may keep sub-agent tools read-only as they are today.
Memory integration should not make parallel sub-agents able to mutate the repo.

## Configuration

Add a `[memory]` table to `entheai.toml`:

```toml
[memory]
enabled = true
strict = false
path = "~/.entheai/memory/entheai.db"
embed_provider = "osaurus"
embed_model = "nomic-embed-text"
retrieve_codebase = 4
retrieve_learnings = 6
retrieve_trajectories = 3
max_context_chars = 12000
tool_spill_chars = 8000
backend = "sqlite-flat"
evidence_tools = ["run_shell", "search"]
```

Defaults:

- `enabled = false` until the integration is stable enough for normal CLI use,
  then flip to true in a later change.
- `strict = false`
- `path = ".entheai/memory.db"` for project-local development unless a global
  path is explicitly configured.
- If no embedder is configured, store/list/get still work and semantic search
  degrades to empty results with a non-fatal diagnostic.

Path handling:

- `entheai-config` stores `path` exactly as TOML text.
- The main binary expands a leading `~/` with the user's home directory before
  opening `SqliteStore`.
- Relative paths are resolved against the canonicalized current working
  directory, matching the built-in tool root.
- The behavior must be tested in config/bin-level helpers without depending on
  the real user's home directory.

## Error handling

- Memory errors are recoverable by default and visible in logs/events.
- Strict mode converts memory errors into task failures.
- Failed post-task writes must not hide the final answer in non-strict mode.
- Tool spill failures fall back to the original tool result so the model still
  has the evidence.
- Malformed metadata is an error during writes and reads; silent drops are not
  allowed.

## Testing

Required coverage for the first implementation:

- `MemoryRuntime` injects a bounded memory system message with source labels.
- Retrieval failure is non-fatal in default mode and fatal in strict mode.
- Large tool output is stored in `tools` and replaced with a memory pointer.
- Small tool output passes through unchanged.
- Final answers write one `trajectories` entry.
- Deterministic extraction writes expected `learnings` metadata.
- Existing `run_task` behavior remains unchanged when memory is disabled.
- CLI config parses `[memory]` defaults and explicit values.
- TUI and fan-out compile with memory disabled.
- Public docs show the same five namespaces as the crate.

The full workspace gate remains:

```bash
./scripts/check.sh
```

For faster iteration:

```bash
cargo test -p entheai-memory
cargo test -p entheai-core
cargo test -p entheai-config
cargo test -p entheai-orchestrator
```

## Rollout

1. Add config and runtime types with memory disabled by default.
2. Add pre-task retrieval and context injection for one-shot CLI.
3. Add tool spillover and final trajectory recording.
4. Wire TUI and fan-out through the same runtime.
5. Update docs and examples.
6. Measure flat search latency on real project memory before adding HNSW.
7. Add MCP-backed `codebase` retrieval.
8. Add linked-note/temporal consolidation as a background task.

## Acceptance

The first complete implementation of this spec is done when:

- A one-shot task can retrieve prior learnings and trajectories before the
  provider call.
- A one-shot task records tool evidence, final trajectory, and durable
  learnings after completion.
- Fan-out records sub-agent outputs in `subagents` and the synthesizer can
  retrieve them.
- Memory failures do not break normal tasks unless `strict = true`.
- Public docs, crate docs, and config examples agree on the five namespaces.
- The targeted memory/core/config/orchestrator tests pass.
- `./scripts/check.sh` passes.

