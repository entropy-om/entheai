# BRAIN v1 — the dyad memory system — Design

## 1. Purpose & role

Deliver an end-to-end, actually-working "brain" for entheai: what a human + LLM
working pair (a dyad) needs from shared memory, in a first shippable slice.
Not a new architecture — entheai already built the pieces (`crates/memory`,
`crates/memory-pp`, frozen nodes, the brain-ring TUI panel) across several
prior sessions. What's missing is that **none of it is wired together in the
one place a human actually experiences it: the interactive TUI session.**
BRAIN v1 closes that gap, then adds the one genuinely new capability this
session's brainstorm converged on: the brain proactively surfacing relevant
context, not just serving retrieval on request.

This work supersedes (for now) the in-flight adk-rust core-loop migration,
which is paused at Task 2/10 (committed to `main`, safe to resume later) per
explicit decision — BRAIN v1 is built against today's `Agent<P>::run_task`,
not the future adk-rust wrapper.

## 2. Current state — what's built vs. what's wired

Three real gaps found by direct investigation (not assumed):

1. **`crates/tui` never calls `run_task_with_memory`** — only plain
   `run_task` (`crates/tui/src/lib.rs:732`, confirmed via a pre-existing
   `TODO(@rahulmranga)` at line 318-323 pointing at
   `docs/superpowers/plans/2026-07-19-entheai-memory-v1.md` → "Task 9"). No
   tool call or transcript turn from an interactive session ever reaches
   `memory-pp`'s `RawStore`. The one-shot CLI path (`bin/entheai/src/main.rs:305`)
   does call `run_task_with_memory` — so memory ingest already works there,
   just not in the TUI, which is where a human actually lives.
2. **`FrozenStore::wake`/`activate` are fully built and tested
   (`crates/memory-pp/src/frozen.rs`, 11 real seed nodes in `frozen/`) but
   called from nowhere** — not `PromptProcessor::retrieve`, not anywhere in
   `bin/entheai` or `crates/tui`. Confirmed via `grep`, zero call sites outside
   the module's own tests.
3. **No proactive layer exists at all.** `FrozenStore::wake(prompt, top_k)`
   is reactive-only: it matches triggers against the *current user message*.
   Nothing today surfaces context the user didn't ask about.

## 3. Architecture

**Slice 1 — close the wiring gaps (baseline, must ship first):**
- `crates/tui`'s `run` loop switches its call at line 732 from `run_task` to
  `run_task_with_memory`, threading through the same `MemoryRuntime`/
  `PromptProcessor`/`MemoryScope` construction `bin/entheai`'s one-shot path
  already does (mirror that wiring, don't reinvent it).
- `PromptProcessor::retrieve` (or the `run_task_with_memory` call site
  immediately around it) gains a `FrozenStore::wake(user_msg, top_k)` call
  ahead of/alongside the existing raw-store recall — a woken node's
  `activate()` output gets prepended to the injected context, same
  "immediately before the last user message" insertion point that already
  exists for the retrieval brief. A woken node also calls
  `BrainState::wake_frozen(name)` so the ring visibly glows for a
  reactively-matched node, not just a proactively-surfaced one.

**Slice 2 — proactive surfacing (the new capability):**
- New `BrainJudge` component (new module, `crates/memory-pp/src/judge.rs`
  or a small new crate if it grows — start in `memory-pp` since it consumes
  `RawStore`/`FrozenStore` directly, no need for a new crate boundary at v1
  size): a background tokio task spawned once at TUI startup, holding a
  channel receiver fed by `PromptProcessor::ingest_tool`/`ingest_transcript`
  (Slice 1 makes these fire for the first time in interactive sessions —
  `BrainJudge` is the second consumer of that same signal).
- On each ingest event, debounced (a fixed cooldown — value TBD at plan
  time, no faster than the frozen-node "melt back" decay `BrainState::tick`
  already animates, so glows don't flicker), `BrainJudge` builds a compact
  prompt from the recent activity window (last N raw-store rows) plus the
  `FrozenStore`'s node names+trigger lists, and asks a local model (via
  `entheai_providers::Provider`, resolved the same "provider/model" way
  everything else in entheai's config already works — osaurus by default,
  no new config surface) which nodes (if any) are relevant right now.
- A matched node → `BrainState::wake_frozen(name)` (visual glow) + a compact
  footer note in the brain panel, same visual language as this session's
  `kx N%` compression-pulse readout (`crates/viz/src/brain.rs`'s
  `footer_line`) — e.g. `👁 nixos` appended when active, absent otherwise.
- Fail-safe by construction, same philosophy as the rest of `memory-pp`:
  provider unreachable, timeout, malformed judgment response, or no
  match — all resolve to "surface nothing," never a hard error, never a
  blocked main loop. The judge runs fully async/detached from the request
  path that's actually answering the user.

## 4. Data flow

- Slice 1: user message → `run_task_with_memory` → (new) `FrozenStore::wake`
  reactive match + existing raw-store recall → both briefs injected before
  the last user message, same convention as today's retrieval injection.
  Tool call / transcript turn → `ingest_tool`/`ingest_transcript` (already
  exist, just now actually invoked from the TUI).
- Slice 2: the same `ingest_tool`/`ingest_transcript` calls additionally
  notify `BrainJudge` (a `tokio::sync::mpsc` channel, or a direct async call
  if simpler — decided at plan time based on whether `PromptProcessor`
  should own the channel sender or the caller should hold both). `BrainJudge`
  reads recent `RawStore` rows + `FrozenStore::nodes()`, calls the local
  model, and on a match calls back into the TUI's `BrainState` (needs a
  shared handle — likely `Arc<Mutex<BrainState>>` or an mpsc channel back to
  the render loop, since `BrainState` today is owned directly by `App` in
  `crates/tui`; exact plumbing is a plan-time decision, not a spec-time one).

## 5. Error handling

- Every new failure mode (frozen store empty, judge's model unreachable,
  malformed judgment output, channel closed) degrades to "nothing surfaces,"
  never an `Err` that reaches the user or blocks a turn. This matches
  `memory-pp`'s existing house style exactly (see `processor.rs`'s own
  doc comment: "Every non-happy branch returns `Ok(None)`... which the core
  call site treats as fall back").
- The judge must not add latency to the user-visible turn: it's spawned as
  a detached background task, not awaited inline anywhere in the
  request/response path.

## 6. Testing

- Slice 1: a TUI-level (or `run_task_with_memory`-level, wherever the wiring
  actually lands) test proving a tool call made during an interactive-style
  run produces a `RawStore` row — today impossible to test because the call
  never happens; this test is the proof the gap is closed.
- Slice 1: a `FrozenStore::wake` integration test proving a triggered node's
  `activate()` output appears in the injected context and
  `BrainState::wake_frozen` gets called — reuse the existing
  `frozen_node_wakes_and_melts` test's assertions style
  (`crates/viz/src/brain.rs`) as the visual-side half of this.
- Slice 2: `BrainJudge` tested against a fake `Provider` (wiremock, matching
  every other provider-calling test in this codebase), covering: a genuine
  match wakes the right node, no-match surfaces nothing, provider timeout/
  error surfaces nothing (fail-safe proof), and debounce actually
  suppresses a second trigger within the cooldown window.

## 7. Scope, non-goals, open questions

**In scope:** the two wiring-gap fixes (Slice 1), `BrainJudge`'s new
event-driven proactive layer (Slice 2), visual surfacing via the existing
brain-ring footer pattern.

**Non-goals:**
- The adk-rust migration (paused separately, resumes independently later).
- Chat-injected surfacing (explicitly declined — visual-only per
  brainstorm decision).
- A human-facing memory review/edit/curation UI (a real "dyad" need
  identified during brainstorming but explicitly deferred — a separate
  future spec, not v1).
- Any new frozen-node authoring tooling — the 11 existing seed nodes are
  the v1 corpus; growing/curating the corpus is out of scope.
- A new crate boundary for `BrainJudge` — starts inside `memory-pp` at this
  size; splitting out is a future refactor if it grows.

**Open questions for the implementation plan:**
- Exact debounce/cooldown duration for `BrainJudge` triggers.
- Exact plumbing from `BrainJudge` (living in `memory-pp`, no TUI
  dependency today) back to `crates/tui`'s `BrainState` (owned by `App`) —
  channel vs. shared `Arc<Mutex<_>>`, decided by reading `crates/tui`'s
  current `App`/event-loop structure at plan time, not guessed here.
- Whether the "recent activity window" is a fixed row count, a time window,
  or both — a plan-time tuning decision, not an architectural one.

## 8. Success criteria

- A tool call made during a real interactive TUI session produces a
  `RawStore` row (Slice 1 gap #1 closed, provable by test).
- A user message matching a frozen-node trigger causes that node's
  knowledge to appear in the model's injected context AND the brain ring
  visibly glows for it (Slice 1 gap #2 closed).
- At least one realistic end-to-end scenario (defined at plan time, e.g.
  "edit a Terraform file, then ask an unrelated question — the `terraform`
  frozen node glows anyway") demonstrates proactive surfacing working
  without the user's own words triggering it.
- No test or manual run shows the judge adding perceptible latency to a
  normal turn, or causing a hard failure when the local model is
  unreachable.
