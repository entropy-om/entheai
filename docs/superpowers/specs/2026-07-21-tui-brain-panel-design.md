# TUI Brain Panel — design

**Goal.** Give the interactive TUI an always-on, compact side panel that shows the agent as a small
living organism: a slowly rotating node-graph of its faculties (model, tools, context) and the remote
fleet, with `wk N · nats ●/○ · ctx %` readouts — woven beside the chat text rather than hidden behind a
full-screen toggle. This makes state that today is invisible (is NATS up? how many workers? is the model
generating or the tool running?) legible at a glance, without leaving the conversation.

**Non-goals (v1).** A live *memory* faculty (no memory event reaches the TUI yet — an open TODO at
`crates/tui/src/lib.rs:267`); replacing the existing full-screen swarm view (`Ctrl-V` stays); touching
the headless CLI path (`bin/entheai`) or the companion.

---

## Architecture

Three pieces, mirroring how the swarm viz already works (a pure model in `crates/viz`, fed by the TUI,
rendered each frame):

1. **`BrainState`** — a new pure, terminal-agnostic model in `crates/viz` (sibling to `SwarmModel`). It
   holds a set of **faculty nodes** with a decaying `activity` level each, a snapshot of **fleet nodes**
   (from the last `list_workers` poll), a `nats_up` bool, and a `ctx_pct`. It exposes small mutators the
   TUI calls from event arms it *already has*, plus a `tick(dt)` that decays activity so a flare fades. No
   tokio, no I/O — unit-testable in isolation, exactly like `SwarmModel`.

2. **`render_brain`** — a renderer (`crates/viz/src/brain.rs`) that draws `BrainState` into a ratatui
   `Rect`: a rotating pseudo-3D point cloud (faculties on an inner ring, fleet on an outer ring, a central
   core), node brightness/size = activity, plus a footer line `wk N · nats ●/○ · ctx P%`. Slice A renders
   with the ratatui `Canvas`/braille path the swarm already uses; Slice B adds a kitty-graphics upgrade.

3. **TUI wiring** (`crates/tui/src/lib.rs`) — a right-hand column carved from the frame, new `App` fields,
   taps on the existing `Token`/`ToolStarted`/`ToolFinished` arms, a throttled fleet poll, and a
   free-running frame counter so the graph rotates even when idle.

### Data flow — what drives each node

Every driver already exists in the event loop; we only *tap* it. No new agent-event channel in v1.

| Node | Driver (existing site) | Effect |
|------|------------------------|--------|
| `model` | `AgentEvent::Token` arm (`lib.rs:680`); `Thinking` (`674`) | flare on each token/thinking burst |
| `tools` | `AgentEvent::ToolStarted` (`696`) / `ToolFinished` (`710`) | flare on call start + result |
| `context` | `est_context_tokens` / `max_context_window` (`1772`/`1755`) | steady fill = ctx %, recomputed per draw |
| fleet (0..N) | throttled `Federation::count_workers`/`list_workers` (`federation:260/241`) | one node per worker; Idle/Working color |
| `nats ●/○` | `bus.is_some()` (`lib.rs:344`) ∥ `fleet_fed.is_some()` (`363`) | connected indicator |

Activity is a `f32` in `[0,1]`: a mutator sets it to `1.0` on an event; `tick(dt)` multiplies it by a
decay factor each frame so it eases back to a dim idle glow. This gives motion tied to real work without
storing event history.

### Throttled fleet poll (never per-frame)

`list_workers`/`count_workers` are async fleet pings (each blocks the loop briefly and hits NATS), so they
must **not** run per-frame. The event loop gains a `tokio::time::interval` at ~1.5 s; on each fire it does
one `count_workers(Duration::from_millis(600))` (when `fleet_fed.is_some()`) and writes the count + a fresh
`nats_up` onto `App`. Between polls the panel shows the cached values. Fail-safe: no federation → `wk 0`,
`nats ○`, no fleet nodes; identical to today when `[federation]`/`[nats]` are off.

### Free-running frame + repaint gating

Today `spinner_frame` only advances while `Status::Working` and draws are `dirty`-gated (`lib.rs:438`,
`807`). An always-on rotating panel needs its own counter: add `brain_frame: u64`, advance it on **every**
`ticker.tick()` and set `dirty = true` — but only when the brain panel is actually visible (enabled *and*
the terminal is wide enough). When the panel is hidden the TUI repaints exactly as it does now (no idle
CPU cost regression). At `tick_ms = 90` (default) that is ~11 fps for a small panel — cheap.

---

## Components & interfaces

### `crates/viz/src/brain.rs` (new) + `model` additions

```rust
/// One faculty of the agent. `activity` is 0..1, decays toward 0 each tick.
pub struct Faculty { pub kind: FacultyKind, pub activity: f32 }
pub enum FacultyKind { Model, Tools, Context }   // Memory deferred (no live event yet)

/// A remote worker as a graph node (snapshot of the last poll).
pub struct FleetNode { pub node_id: String, pub working: bool }

pub struct BrainState {
    pub faculties: Vec<Faculty>,   // Model, Tools, Context — constructed in ::new()
    pub fleet: Vec<FleetNode>,
    pub nats_up: bool,
    pub worker_count: usize,
    pub ctx_pct: u16,
    pub frame: u64,
}

impl BrainState {
    pub fn new() -> Self;                       // three faculties at activity 0
    pub fn flare(&mut self, k: FacultyKind);    // set that faculty's activity = 1.0
    pub fn tick(&mut self);                      // frame += 1; decay every faculty's activity
    pub fn set_fleet(&mut self, workers: &[(String, bool)]); // (node_id, working) → rebuild fleet + worker_count
    pub fn set_nats(&mut self, up: bool);
    pub fn set_ctx_pct(&mut self, pct: u16);
}
```

`set_fleet` takes `&[(String, bool)]` = `(node_id, working)` — **not** `&[WorkerPresence]` — so `crates/viz`
stays pure and gains **no** dependency on `entheai-federation` (which would pull heavy `async-nats` into the
render crate and invert the layering). The `WorkerPresence → (node_id, working)` mapping lives in the TUI's
fleet-poll code: `matches!(w.state, entheai_federation::WorkerState::Working { .. })`.

```rust
// crates/viz/src/brain.rs
pub fn render(state: &BrainState, area: Rect, buf: &mut Buffer, marker: Marker);
```

Layout math: faculties placed on an inner circle, fleet on an outer circle, projected through a small
rotation about the vertical axis by angle `frame * ω` (pseudo-3D: `x' = x·cosθ − z·sinθ`, depth `z` scales
brightness so back nodes dim). A central "core" point links to every faculty (canvas lines, like the
swarm's orchestrator lines). The footer (bottom row of `area`) is a ratatui `Line`:
`wk {worker_count} · nats {●|○} · ctx {ctx_pct}%`, colored like the existing `context_line` thresholds.

### `App` new fields (`crates/tui/src/lib.rs:183`)

```rust
brain: entheai_viz::BrainState,   // constructed in App init (parallels `swarm`)
brain_enabled: bool,              // from config.viz.brain; toggled by /brain
brain_frame: u64,                 // free-running rotation counter
nats_up: bool,                    // cached from the fleet poll (and bus.is_some at startup)
worker_count: usize,              // cached from the fleet poll
```

### Config (`crates/config/src/lib.rs:200`, `VizConfig`)

```rust
#[serde(default = "default_viz_brain")]       pub brain: bool,       // default true
#[serde(default = "default_viz_brain_width")] pub brain_width: u16,  // default 26 (columns)
```
Following the existing `#[serde(default = "...")]` + `default_*` fn + `impl Default` pattern verbatim.

### The horizontal split (`render`, `lib.rs:1571`)

At `let area = frame.area();`, before the `ViewMode` branch:

```rust
let show_brain = app.brain_enabled && area.width >= MIN_WIDTH_FOR_BRAIN; // e.g. 72
let (body_area, brain_area) = if show_brain {
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(config_brain_width)]).areas(area);
    (l, Some(r))
} else { (area, None) };
```

`body_area` feeds *both* existing branches (the full-screen swarm `Layout::vertical(...).areas(body_area)`
and the chat `Layout::vertical(...).areas(body_area)`) unchanged. When `brain_area` is `Some`, draw a
bordered ` brain ` block and `entheai_viz::brain::render` into its inner rect. So the panel coexists with
both views; when the terminal is narrow it silently disappears and the layout is byte-identical to today.

### `/brain` command

A local command (like `/viz`): toggles `app.brain_enabled`, mirrors `is_viz_command`/`handle_viz_command`
(`lib.rs:1118`). Added to the slash-menu table (`lib.rs:1816` neighborhood) and the help text.

---

## Rendering slices

**Slice A — braille pseudo-3D + full data wiring (ships first, works everywhere).**
The rotating point cloud, faculties/fleet/core, activity brightness, the footer readouts, the split, the
config, `/brain`, the event taps, the throttled poll, the free-running frame. Marker `Braille` (fallback
to `HalfBlock` is a one-line choice). This is the "ASCII point-cloud fallback" the chosen option includes,
and it is a complete, useful feature on its own.

**Slice B — kitty-graphics 3D upgrade (gated).**
When `entheai_viz::term::graphics_capable()` is true (kitty/ghostty/wezterm), replace the braille cloud
with a true-3D render emitted via the Kitty graphics protocol into the panel's cell region, redrawn each
frame. This is net-new (no pixel rendering exists in the crate today) and interacts delicately with
ratatui owning the buffer — hence it is sequenced second, behind a probe, with the Slice-A braille render
as the guaranteed fallback for every non-kitty terminal. Deferrable without weakening Slice A.

---

## Error handling & fail-safes

- **Narrow terminal** → panel auto-hides (`area.width < MIN_WIDTH_FOR_BRAIN`); no layout change vs today.
- **Federation/NATS off or unreachable** → poll yields `wk 0 · nats ○`, no fleet nodes; unchanged behavior.
- **Fleet poll latency** → runs on its own interval off the render path; a slow/failed `list_workers`
  never stalls a frame (cached values persist).
- **Graphics probe false / Slice B absent** → braille render (Slice A) always available.
- **`/brain` off** → free-running frame + forced `dirty` stop; idle CPU returns to today's baseline.

## Testing

Pure-model unit tests in `crates/viz` (no terminal needed), matching the `SwarmModel` test style:
- `flare` sets activity to 1.0; `tick` decays it monotonically toward 0 and never below 0.
- `set_fleet` builds one `FleetNode` per presence, maps `WorkerState::Working` → `working=true`, and sets
  `worker_count`; empty slice → empty fleet, `worker_count = 0`.
- `set_nats` / `set_ctx_pct` round-trip.
- projection helper: a node at depth `+z` renders dimmer than the same node at `−z` (brightness monotonic
  in depth), and rotation is periodic in `frame`.
- `render` smoke test: drawing into a small `Buffer` does not panic and writes the footer string.

TUI-side: a `brain_rows`/`show_brain` gate helper (pure, like `swarm_rows_for` at `lib.rs:847`) gets a unit
test for the width threshold and enabled flag. Integration is verified by a headless build + a real launch
(Ghostty tier is computer-use "click" — no typing — so verify via `--no-companion "<prompt>"` one-shot and
visual inspection, not synthetic input).

## File structure

- **Create** `crates/viz/src/brain.rs` (model + render) — or split model into `model.rs` additions +
  `brain.rs` render; the plan will pick one. Re-export from `crates/viz/src/lib.rs:10`. (No new crate
  dep — `viz` stays pure; the `WorkerPresence` mapping happens in the TUI.)
- **Modify** `crates/config/src/lib.rs` (`VizConfig` + defaults).
- **Modify** `crates/tui/src/lib.rs` (App fields, init, the split in `render`, the panel draw, event-arm
  taps, the fleet-poll interval, the free-running frame, `/brain` command + help + slash-menu entry).
- **Modify** `entheai.toml` (document `[viz] brain` / `brain_width`).
- **Docs**: CHANGELOG entry.

## Scope check

One subsystem (the TUI panel + its pure model), one implementation plan. Slice B (kitty graphics) is a
clean follow-on that does not block Slice A. Memory-as-faculty is deferred until the memory-into-run-loop
event lands (`lib.rs:267` TODO) — at which point it is a one-node addition.
