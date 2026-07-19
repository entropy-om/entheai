# entheai TUI — Flow, Transparency & Feel — Design

**Date:** 2026-07-19 · **Status:** approved design, pre-plan
**Scope:** the first of the **five v0.2 pillars** (this TUI pillar · codebase-memory · durable sessions · Sonar · viz). The other four get their own specs.

**Inspiration:** **Crush** — one seamless session in a single terminal view, polished animation, and a smooth prompt → approve → result flow you can *slide into*; **jcode** — extreme Rust performance (≈10 sessions in the memory of half a Claude Code). This pillar makes entheai's TUI *feel* like that.

## 1. Purpose

Turn the TUI into a **single-window flow state**: everything (chat, the agent's live plan, progress, permission prompts, input) in one cohesive, animated view; you can *see* what the agent is doing and *progressively* hand it the keys until you're in a YOLO-like flow — without ever restarting. Concretely, four threads:

1. an enriched **progress line** (verb · elapsed · token tally · effort);
2. a live **plan pane** the agent maintains;
3. a **permission flow** that offers *allow-for-session*, so trust accretes into flow;
4. an **animated, ultra-fast feel** (Crush animation + jcode performance).

## 2. Progress line (enriched)

```
⠙ Churning… · 4m31s · ↓18.4k tokens · med
```
- **Verb** — a rotating status word replacing flat "thinking": cycle `[Thinking, Churning, Weaving, Reasoning, Wrangling, Cooking, Threading, Brewing]` **by turn index** (deterministic — no `Date::now`/`rand`). A running tool still overrides it (`running read_file`).
- **Elapsed** — the existing `run_started.elapsed()`.
- **↓ tokens** — cumulative **output** tokens for the current run, tallied **client-side** from `AgentEvent::Token` deltas (approx `bytes/4`), reset on submit. `18_432→"18.4k"`, `950→"950"`, `1_250_000→"1.2M"`.
- **Effort** — shown only when a run declares one; **omitted by default** (no universal effort metric). Forward-looking.

## 3. Plan pane

A bordered `plan` region **between the scrollback and the progress line**:
```
╭ plan ──────────────────────────╮
│ ✓ read Cargo.toml              │
│ ◐ map the auth module          │
│ ◻ add the retry helper         │
╰────────────────────────────────╯
```
- Markers: `◻` pending · `◐` in_progress · `✓` done · `✗` failed.
- **Empty plan → 0 rows** (region collapses; input never jumps when idle).
- Height `min(items+2, 10)`; overflow → first rows + `… +N more`; rows truncated to width.

## 4. Where the steps come from (both)

- **`todo` tool (agent-authored)** — schema `{ items: [{ text, status: pending|in_progress|done|failed }] }`; a call **replaces** the plan. Registered + advertised in the system prompt. The TUI reads it straight off `AgentEvent::ToolStarted{name:"todo", args}` (parse `args.items`) — **no new core event required**; `TodoTool::call` just returns `"plan: N items"`.
- **Fan-out (automatic)** — via the existing `FanoutEvent`s: `Decomposed` seeds `pending` rows, `CoderStarted` → `◐`, `CoderFinished` → `✓`/`✗`. `Decomposed`/`CoderStarted` carry the sub-task `(role, task)` text to label rows.

## 5. Permission flow — sliding into YOLO

Started **without** `--yolo`, gated tool calls raise a modal with **three** choices, not two:
```
allow write_file(src/retry.rs)?   [y]es · [n]o · [a]llow for session
```
- **[y]es** — allow this one call.
- **[n]o** — deny (the model receives the denial text and adapts).
- **[a]llow for session** — allow *this tool* for the rest of the run; never asked for it again. Progressive: allow `read_file`, then `search`, then `edit_file`, then `run_shell` as trust grows → you're in a YOLO-like flow **without restarting**. `--yolo` stays the "allow everything up front" shortcut.

**Mechanics.** `Prompter::confirm` returns a 3-way `Decision { Deny, Allow, AllowSession }` (today it returns `bool`). On `AllowSession` the agent loop inserts the tool name into a **mutable per-session allowlist** (e.g. `Arc<Mutex<HashSet<String>>>` threaded into `dispatch_call`, consulted before prompting — analogous to `Policy::decide`'s `allowlist`, but grown at runtime). Granularity v1: **per tool name**; per-command patterns for `run_shell` are a later refinement. This is an additive change to `crates/permission` (the `Decision`/`Prompter` return) + `crates/core` (consult + grow the session set) + `crates/tui` (the 3-option modal + the `StdinPrompter` gets a matching 3-way prompt).

## 6. Feel — animation (Crush) + performance (jcode)

- **Single-window cohesion** — chat, plan, progress, permission modal, and input are one view; no panes/tabs to manage. The plan pane and permission modal appear/collapse **inline**, never spawning separate surfaces.
- **Animation** — frame-based, Bubbletea-style: the braille spinner, a streaming reveal/cursor on the assistant bubble, the plan pane's appear/collapse, and the permission modal use smooth per-frame transitions (a few frames of grow/fade where cheap). One consistent tick drives it.
- **Performance (jcode-grade)** — the render loop is **frame-budgeted** and **pauses when idle** (only redraw on a real change or an active animation — no flat-out polling), with **no per-frame allocation** in the hot path: pre-wrapped history lines are cached and rebuilt only when content or width changes. Target: negligible idle CPU, instant input.

## 7. Architecture & data flow

- **`crates/permission`** — `Decision { Deny, Allow, AllowSession }`; `Prompter::confirm` returns it; `StdinPrompter` prints the 3-way prompt.
- **`crates/core`** — `TodoItem { text, status }` + `TodoTool`; a **mutable session allowlist** (`Arc<Mutex<HashSet<String>>>`) threaded into `dispatch_call`, consulted before `policy.decide` and grown on `AllowSession`. (Optionally emit `AgentEvent::Plan` when the dispatched tool is `todo`; else the TUI reads it off `ToolStarted`.)
- **`crates/tui`** — new state: `plan: Vec<TodoItem>`, `out_tokens: usize`, `verb_idx: usize`, `line_cache`. New vertical region `PLAN_ROWS` (0 when empty): `status / history / plan / progress / input`. The permission modal grows to 3 options; the key handler maps `y/n/a`. Cache wrapped lines; redraw only on change or active animation.
- **`crates/orchestrator`** — `FanoutEvent::Decomposed`/`CoderStarted` carry sub-task text for row labels.

## 8. Testing

- **Pure (tui):** plan renderer (each marker; empty→0 rows; truncation; `… +N more`); token formatter (`18_432→"18.4k"`, `950→"950"`, `1_250_000→"1.2M"`); verb rotation (deterministic, wraps); `todo` args parser (valid → `Vec<TodoItem>`; bad status → `pending`; malformed → empty, no panic); the line-cache invalidation (rebuild on width/content change only).
- **permission/core:** the 3-way `Decision`; a `Prompter` returning `AllowSession` adds the tool to the session set so the *second* call of that tool is not prompted (assert via the recording-provider harness).
- **tools/core:** the `todo` tool (schema + confirmation).
- **Manual:** live run shows verb + growing token tally; `todo` renders the pane; `--fanout` auto-populates it; a non-yolo run shows y/n/a and "allow for session" stops re-prompting that tool; idle CPU stays low.

## 9. Scope · non-goals

**In:** enriched progress line · plan pane · `todo` tool · fan-out→plan mapping · 3-way permission flow (allow-for-session) · animation + idle-frugal render.
**Non-goals (v1):** exact provider-reported token accounting (client approximation is fine); a universal effort metric (optional); per-command `run_shell` permission patterns (per-tool for now); interactive/collapsible plan; persisting the plan or the session allowlist across restarts (that's *durable sessions*); pixel/GPU animation (that's *viz*).

## 10. Success criteria

In one seamless window: the progress line shows a rotating verb + elapsed + a growing `↓` token count; a `plan` pane appears with `◻/◐/✓/✗` steps (from `todo` or a fan-out) and collapses to zero rows when idle; a non-yolo run offers **[y]es · [n]o · [a]llow for session**, and choosing *allow for session* stops re-prompting that tool for the rest of the run — so you slide into a flow state; and the render loop stays animated yet idle-frugal (no busy-spin).
