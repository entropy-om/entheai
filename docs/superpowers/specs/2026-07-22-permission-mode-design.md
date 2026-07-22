# Permission + Mode — design

## The idea

Give entheai a single, legible **permission posture** the operator cycles with
`Shift+Tab` — `plan · auto · yolo · ask` — that governs the whole run, main agent
and fan-out subagents alike. Today permission is a flat allowlist plus a binary
`yolo`; there is no "planning, don't touch anything" state, no risk distinction
between reading a file and running a shell command, and subagents are hard-wired to
auto-approve. This design adds tool **risk tiers**, a runtime **mode**, per-tool
**pins**, and **subagent propagation** — a coherent model instead of scattered flags.

## Current state (what we're extending)

- `crates/permission`: `Policy{yolo, allowlist, session}` → `decide(tool) ->
  Decision{Allow, Deny, Ask}` (today only `Allow`/`Ask` are ever returned). `Ask`
  is resolved by a `Prompter` (CLI stdin / TUI modal) into `Grant{Allow,
  AllowSession, Deny}`.
- Subagents: `orchestrator::fanout_policy` = `Policy::new(fanout_auto_approve,
  allowlist)` + an `AutoAllow` prompter; the agy executor passes
  `--dangerously-skip-permissions`. Effectively yolo, unattended.
- Tools: a flat name allowlist — **no risk classification**.

## Architecture

### 1. Tiers — `crates/permission`

```rust
pub enum Tier { Read, Write, Exec, Network, Spawn }
```

`Read` (list/read files, search), `Write` (edit/create/delete files), `Exec`
(shell/process), `Network` (fetch/HTTP/MCP-over-network), `Spawn` (launch a
subagent / fan-out). Ordered by autonomy: `Read < Write < Exec < Network < Spawn`.

**Classification.** Built-in tools self-declare `fn tier(&self) -> Tier` on the tool
trait in `crates/tools`. A central override table (name → tier) in `crates/permission`
covers cases the trait can't reach. **Unknown / MCP / dynamic tools default to
`Exec`** — conservative, because an unclassified tool could do anything. Config pins
override per tool.

### 2. Mode + pins — `Policy`

```rust
pub enum Mode { Plan, Auto, Yolo, Ask }
pub enum Pin  { AlwaysAllow, AlwaysAsk, Never }   // Never == always Deny
```

`Policy` gains a runtime `mode: Arc<Mutex<Mode>>` (interior mutability, like the
existing session set, so the TUI toggles it live) and `pins: HashMap<String, Pin>`
(from config). New decision entry point:

```rust
pub fn decide_tiered(&self, tool: &str, tier: Tier) -> Decision;
```

Resolution order: **pin** (AlwaysAllow→Allow, AlwaysAsk→Ask, Never→Deny) → else the
**mode × tier matrix**. `decide(tool)` stays as a thin shim (`decide_tiered(tool,
classify(tool))`) so existing callers keep working; `yolo`/`fanout_auto_approve`
become compatibility shims that map onto `Mode::Yolo` / a subagent ceiling.

### 3. Mode × Tier matrix (interactive / main agent)

| mode \ tier | Read | Write | Exec | Network | Spawn |
|---|---|---|---|---|---|
| **plan** | Allow | Deny | Deny | Deny | Deny |
| **auto** | Allow | Allow | Ask | Ask | Ask |
| **yolo** | Allow | Allow | Allow | Allow | Allow |
| **ask** | Allow | Ask | Ask | Ask | Ask |

- `Deny` → the tool call is refused with a short reason ("denied by plan mode");
  the agent receives that as the tool result and keeps reasoning — which naturally
  yields a plan. No special plan-report format (YAGNI).
- `Ask` → the existing `Prompter` modal, unchanged.
- `Allow` → runs.

`auto` is "edit freely, check before *running* things"; `ask` is "check before any
change"; `plan` is read-only; `yolo` is full send.

### 4. Subagents — inherited tier ceiling, overridable per-run

Fan-out subagents are unattended and cannot prompt, so they resolve to a **tier
ceiling** rather than the interactive matrix. The parent mode propagates via env
`ENTHEAI_MODE` (mirroring `ENTHEAI_FANOUT_DEPTH`); the child builds its policy from it:

| parent mode | subagent ceiling |
|---|---|
| plan | `Read` |
| auto | `Exec` |
| ask  | `Exec` |
| yolo | `Spawn` (all) |

A subagent auto-approves any tool with `tier ≤ ceiling`, denies above — never asks.
So **plan mode stops the whole tree** (subagents can only read); coders still
build/test under auto/ask (Exec); yolo is unrestricted. A fan-out run may **override**
the ceiling (`--fanout-mode <mode>` / `[fanout] mode`), per the "overridable per-run"
decision — e.g. run coders in `yolo` while the parent stays in `ask`. The agy
executor's `--dangerously-skip-permissions` is gated on the resolved ceiling being
`Spawn`/all; below that it passes a bounded policy instead.

### 5. TUI toggle + display — `crates/tui`

`Shift+Tab` (`KeyCode::BackTab`, plus `Tab`+SHIFT fallback) cycles
`plan → auto → yolo → ask → plan`. A colored `mode: <label>` segment in the status
line (plan=cyan, auto=green, yolo=red, ask=yellow). The handler writes the shared
`Policy` mode via its `Arc<Mutex<Mode>>`, so the change takes effect immediately for
in-flight and subsequent tool calls. The existing `AwaitingPermission` modal remains
the resolver for `Ask` cells.

### 6. Config + persistence — `crates/config`

```toml
[permission]
mode = "ask"                      # startup default: plan | auto | yolo | ask
pins = { run_shell = "always_ask", read_file = "always_allow" }
# yolo / fanout_auto_approve retained as compatibility shims
[fanout]
mode = ""                         # "" = inherit parent ceiling; else override
```

Runtime `Shift+Tab` changes are **session-only** (not written back — YAGNI). A typo
in `mode` warns and falls back to `ask` (fail-safe, mirroring `RetrievalMode::parse`).

## Error handling & fail-safes

- Unknown tool tier → `Exec` (conservative), never `Read`.
- Poisoned mode lock → recover the inner value (never panic the agent), like the
  session set today.
- Subagent with no `ENTHEAI_MODE` env (direct worker launch) → defaults to the
  `auto` ceiling (`Exec`) — matches today's `fanout_auto_approve` behavior for coders.
- `plan` denial is a normal tool result, not an error — the run continues.

## Testing

- **Matrix:** table-driven unit tests over every `(mode, tier)` cell → expected
  `Decision`. Pins override the matrix (each `Pin` variant).
- **Classification:** each built-in tool returns its declared tier; unknown/MCP →
  `Exec`; config pins win.
- **Subagent ceiling:** `ceiling(mode)` mapping; `tier ≤ ceiling → Allow`, above →
  `Deny`; per-run override replaces the ceiling; missing env → `auto` ceiling.
- **TUI:** `Mode::next()` cycle order; `status_line` shows the colored mode; a
  `BackTab` key event advances the mode and updates the shared policy; an incoming
  `Ask`-cell request pops the modal while an `Allow`/`Deny` cell auto-resolves.
- **Compat:** `yolo = true` ⇒ `Mode::Yolo` behavior; `decide(tool)` shim matches the
  old semantics for allowlisted/session tools.

## Phasing (the plan will sequence these)

1. **Core Policy + matrix** — `Tier`, `Mode`, `Pin`, `decide_tiered`, the matrix, the
   `decide()` shim + compat. Fully unit-tested; no behavior change until wired.
2. **Tool tiering** — self-declared `tier()` on built-in tools + the central table +
   `Exec` default; route tool execution through `decide_tiered`.
3. **Subagent ceiling** — `ENTHEAI_MODE` propagation, `ceiling(mode)`, per-run
   override, agy-executor gating.
4. **TUI toggle** — `Shift+Tab`, the status segment, the shared-mode write, modal
   preserved for `Ask`.
5. **Config** — `[permission] mode`/`pins`, `[fanout] mode`, compat shims, docs.

Each phase is independently testable; phase 1 is inert until phase 2 routes through it.

## Scope check

One subsystem (the permission policy) extended along three axes — tiers, mode, pins —
plus its propagation to subagents and a TUI control. A single spec; the plan sequences
it into the five phases above. Out of scope (YAGNI): persisting runtime mode changes,
a bespoke plan-mode report format, per-tool tier configuration in the UI.
