# TUI Brain Panel (Slice A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an always-on compact side panel to the interactive TUI — a slowly rotating braille pseudo-3D node graph of the agent's faculties (model, tools, context) plus the remote fleet, with a `wk N · nats ●/○ · ctx %` footer — woven beside the chat text.

**Architecture:** A pure, terminal-agnostic `BrainState` model + `render` fn in a new `crates/viz/src/brain.rs` (sibling to `swarm.rs`), fed by the TUI from event arms it already has (`Token`/`Thinking` → model flare, `ToolStarted`/`ToolFinished` → tools flare), a throttled ~1.5 s fleet poll (`Federation::list_workers`) writing fleet/nats into `app.brain`, and a horizontal split carved at `render()`. Rotation + activity decay live in `BrainState` (`frame`, per-faculty `activity`), advanced by `brain.tick()` each animation tick. `viz` gains **no** new crate deps — the `WorkerPresence → (id, working)` mapping happens in the TUI.

**Tech Stack:** Rust, ratatui (`widgets::canvas::Canvas`, `Marker::Braille`), tokio `interval`. Crates touched: `entheai-viz`, `entheai-config`, `entheai-tui`.

**Scope:** Slice A only. Slice B (kitty-graphics true-3D upgrade behind `graphics_capable()`) is a deferred follow-on; the braille render here is its guaranteed fallback. A live `memory` faculty is deferred until memory events reach the TUI (`crates/tui/src/lib.rs:267` TODO).

**Refinement over the spec:** `frame`, `worker_count`, `nats_up`, `ctx_pct` live inside `BrainState`; `App` gains only `brain: BrainState` + `brain_enabled: bool`.

---

### Task 1: `BrainState` pure model (`crates/viz/src/brain.rs`)

**Files:**
- Create: `crates/viz/src/brain.rs`
- Modify: `crates/viz/src/lib.rs` (declare `pub mod brain;` + re-export types)
- Test: inline `#[cfg(test)] mod tests` in `crates/viz/src/brain.rs`

- [ ] **Step 1: Write the failing tests**

Add to the bottom of `crates/viz/src/brain.rs` (create the file with just this test module first — it won't compile until Step 3 adds the types; that's the intended red):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flare_sets_full_activity_then_decays_bounded() {
        let mut b = BrainState::new();
        assert_eq!(b.faculty(FacultyKind::Model).activity, 0.0);
        b.flare(FacultyKind::Model);
        assert_eq!(b.faculty(FacultyKind::Model).activity, 1.0);
        b.tick();
        let a1 = b.faculty(FacultyKind::Model).activity;
        assert!(a1 < 1.0 && a1 >= 0.0, "decays and stays non-negative: {a1}");
        for _ in 0..200 { b.tick(); }
        let a = b.faculty(FacultyKind::Model).activity;
        assert!((0.0..0.02).contains(&a), "eases to ~0: {a}");
    }

    #[test]
    fn tick_advances_frame() {
        let mut b = BrainState::new();
        assert_eq!(b.frame, 0);
        b.tick();
        assert_eq!(b.frame, 1);
    }

    #[test]
    fn set_fleet_maps_working_and_counts() {
        let mut b = BrainState::new();
        b.set_fleet(&[("aaa".to_string(), true), ("bbb".to_string(), false)]);
        assert_eq!(b.worker_count, 2);
        assert_eq!(b.fleet.len(), 2);
        assert!(b.fleet[0].working);
        assert!(!b.fleet[1].working);
        b.set_fleet(&[]);
        assert_eq!(b.worker_count, 0);
        assert!(b.fleet.is_empty());
    }

    #[test]
    fn nats_and_ctx_round_trip() {
        let mut b = BrainState::new();
        b.set_nats(true);
        b.set_ctx_pct(42);
        assert!(b.nats_up);
        assert_eq!(b.ctx_pct, 42);
    }

    #[test]
    fn projection_periodic_and_depth_monotonic() {
        let period = (2.0 * std::f64::consts::PI / OMEGA).round() as u64;
        let (x0, y0, _) = project(0.0, 0.5, 0.0, 0);
        let (xp, yp, _) = project(0.0, 0.5, 0.0, period);
        assert!((x0 - xp).abs() < 2e-2 && (y0 - yp).abs() < 2e-2, "one rotation returns near start");
        assert!(depth_brightness(0.4, 0.5) > depth_brightness(-0.4, 0.5), "nearer = brighter");
        let db = depth_brightness(0.0, 0.5);
        assert!((0.0..=1.0).contains(&db));
    }
}
```

- [ ] **Step 2: Run tests, verify they fail to compile**

Run: `cargo test -p entheai-viz brain`
Expected: compile error (`BrainState` etc. not found) — the intended red.

- [ ] **Step 3: Write the model (top of `crates/viz/src/brain.rs`, above the test module)**

```rust
//! Always-on "brain state" panel model + braille render (Slice A).
//!
//! A small living graph: the agent's faculties (model, tools, context) on an inner
//! ring, the remote fleet on an outer ring, a central core — slowly rotating, node
//! brightness driven by decaying per-faculty activity. Pure + terminal-agnostic;
//! fed by the TUI from event arms it already has. See
//! docs/superpowers/specs/2026-07-21-tui-brain-panel-design.md.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Context, Line as CanvasLine};
use ratatui::widgets::Widget;

/// Rotation speed, radians per animation tick (~11 fps at tick_ms=90).
const OMEGA: f64 = 0.06;
/// Per-tick activity decay factor (flare eases to a dim glow).
const DECAY: f32 = 0.90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacultyKind { Model, Tools, Context }

#[derive(Debug, Clone)]
pub struct Faculty {
    pub kind: FacultyKind,
    /// 0..1, set to 1.0 on a `flare`, multiplied by `DECAY` each `tick`.
    pub activity: f32,
}

/// A remote worker as a graph node (snapshot of the last fleet poll).
#[derive(Debug, Clone)]
pub struct FleetNode {
    pub node_id: String,
    pub working: bool,
}

#[derive(Debug, Clone)]
pub struct BrainState {
    pub faculties: Vec<Faculty>,
    pub fleet: Vec<FleetNode>,
    pub nats_up: bool,
    pub worker_count: usize,
    pub ctx_pct: u16,
    pub frame: u64,
}

impl Default for BrainState {
    fn default() -> Self { Self::new() }
}

impl BrainState {
    pub fn new() -> Self {
        BrainState {
            faculties: vec![
                Faculty { kind: FacultyKind::Model, activity: 0.0 },
                Faculty { kind: FacultyKind::Tools, activity: 0.0 },
                Faculty { kind: FacultyKind::Context, activity: 0.0 },
            ],
            fleet: Vec::new(),
            nats_up: false,
            worker_count: 0,
            ctx_pct: 0,
            frame: 0,
        }
    }

    pub fn faculty(&self, kind: FacultyKind) -> &Faculty {
        self.faculties.iter().find(|f| f.kind == kind).expect("faculty exists")
    }

    pub fn flare(&mut self, kind: FacultyKind) {
        if let Some(f) = self.faculties.iter_mut().find(|f| f.kind == kind) {
            f.activity = 1.0;
        }
    }

    /// Advance rotation + decay every faculty's activity toward 0.
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        for f in &mut self.faculties {
            f.activity = (f.activity * DECAY).max(0.0);
        }
    }

    pub fn set_fleet(&mut self, workers: &[(String, bool)]) {
        self.fleet = workers
            .iter()
            .map(|(id, working)| FleetNode { node_id: id.clone(), working: *working })
            .collect();
        self.worker_count = self.fleet.len();
    }

    pub fn set_nats(&mut self, up: bool) { self.nats_up = up; }
    pub fn set_ctx_pct(&mut self, pct: u16) { self.ctx_pct = pct; }
}

/// Project a node on a ring (radius `r`, vertical offset `y_off`) rotating about
/// the vertical axis by `frame`. Returns (screen_x, screen_y, depth) in canvas
/// units; `x`/`y` land in roughly [-1, 1], `depth` = world z (front positive).
fn project(angle: f64, r: f64, y_off: f64, frame: u64) -> (f64, f64, f64) {
    let theta = angle + frame as f64 * OMEGA;
    let wx = r * theta.cos();
    let wz = r * theta.sin();
    let sx = wx;
    let sy = y_off - wz * 0.35; // depth tilts vertical position → ring reads as an ellipse
    (sx, sy, wz)
}

/// Nearer nodes (larger z) are brighter; result in [0.35, 1.0], monotonic in z.
fn depth_brightness(wz: f64, r: f64) -> f32 {
    let t = ((wz / r.max(1e-6)) + 1.0) / 2.0; // z=-r→0, z=+r→1
    (0.35 + 0.65 * t) as f32
}
```

- [ ] **Step 4: Declare + re-export in `crates/viz/src/lib.rs`**

At the top module declarations (near `pub mod model; swarm; term;`) add `pub mod brain;`, and extend the re-export line (`crates/viz/src/lib.rs:10`):

```rust
pub use brain::{BrainState, Faculty, FacultyKind, FleetNode};
```

- [ ] **Step 5: Run tests, verify they pass**

Run: `cargo test -p entheai-viz brain`
Expected: 5 tests pass. (The `render` smoke test comes in Task 2.)

- [ ] **Step 6: Commit**

```bash
git add crates/viz/src/brain.rs crates/viz/src/lib.rs
git commit -m "feat(viz): BrainState model — faculties, activity decay, fleet snapshot, rotation"
```

---

### Task 2: Braille pseudo-3D `render` (`crates/viz/src/brain.rs`)

**Files:**
- Modify: `crates/viz/src/brain.rs` (add `render` + `footer_line` + helpers; add a smoke test)

- [ ] **Step 1: Write the failing smoke test** (add to the `tests` module)

```rust
    #[test]
    fn render_small_buffer_no_panic_and_footer() {
        let mut b = BrainState::new();
        b.set_nats(true);
        b.set_ctx_pct(42);
        b.set_fleet(&[("n1".to_string(), true)]);
        b.flare(FacultyKind::Tools);
        let area = Rect::new(0, 0, 26, 12);
        let mut buf = Buffer::empty(area);
        render(&b, area, &mut buf, Marker::Braille);
        // read the footer row
        let y = area.bottom() - 1;
        let mut row = String::new();
        for x in area.left()..area.right() {
            row.push_str(buf[(x, y)].symbol()); // ratatui indexing; mirror swarm.rs's buffer access
        }
        assert!(row.contains("wk 1"), "footer worker count: {row:?}");
        assert!(row.contains("42%"), "footer ctx pct: {row:?}");
        assert!(row.contains('●'), "nats up marker: {row:?}");
    }
```

Note: use whatever buffer-cell access the crate's ratatui version uses — check `crates/viz/src/swarm.rs` for the exact API (`buf[(x,y)].symbol()` vs `buf.cell((x,y))`) and match it.

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p entheai-viz brain::tests::render_small_buffer`
Expected: compile error (`render` not found).

- [ ] **Step 3: Implement `render` + `footer_line`** (add to `brain.rs`, after the model)

```rust
/// Draw the brain panel into `area`: a rotating canvas (all rows but the last) +
/// a `wk N · nats ●/○ · ctx P%` footer on the bottom row.
pub fn render(state: &BrainState, area: Rect, buf: &mut Buffer, marker: Marker) {
    if area.width < 4 || area.height < 2 {
        return;
    }
    let canvas_area = Rect { height: area.height - 1, ..area };
    let n_fac = state.faculties.len().max(1);
    let n_fleet = state.fleet.len().max(1);

    let canvas = Canvas::default()
        .marker(marker)
        .x_bounds([-1.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(|ctx: &mut Context| {
            // core → faculty links (dim, back layer)
            for (i, _f) in state.faculties.iter().enumerate() {
                let a = i as f64 / n_fac as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 0.45, 0.10, state.frame);
                let g = (depth_brightness(wz, 0.45) * 90.0) as u8;
                ctx.draw(&CanvasLine { x1: 0.0, y1: 0.0, x2: x, y2: y, color: Color::Rgb(0, g, g) });
            }
            ctx.layer();
            // core
            ctx.print(0.0, 0.0, Span::styled("✦", Style::default().fg(Color::Rgb(120, 200, 220))));
            // faculties (inner ring)
            for (i, f) in state.faculties.iter().enumerate() {
                let a = i as f64 / n_fac as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 0.45, 0.10, state.frame);
                let db = depth_brightness(wz, 0.45);
                let v = ((0.30 + 0.70 * f.activity) * db * 255.0) as u8;
                let glyph = match f.kind { FacultyKind::Model => "M", FacultyKind::Tools => "T", FacultyKind::Context => "C" };
                ctx.print(x, y, Span::styled(glyph, Style::default().fg(Color::Rgb(0, v, v))));
            }
            // fleet (outer ring)
            for (i, node) in state.fleet.iter().enumerate() {
                let a = i as f64 / n_fleet as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 0.85, -0.12, state.frame);
                let db = depth_brightness(wz, 0.85);
                let base = if node.working { (0u8, 220, 90u8) } else { (90u8, 90, 90u8) };
                let col = Color::Rgb(
                    (base.0 as f32 * db) as u8,
                    (base.1 as f32 * db) as u8,
                    (base.2 as f32 * db) as u8,
                );
                ctx.print(x, y, Span::styled("•", Style::default().fg(col)));
            }
        });
    Widget::render(canvas, canvas_area, buf);

    let footer = footer_line(state);
    buf.set_line(area.x, area.bottom() - 1, &footer, area.width);
}

fn footer_line(state: &BrainState) -> Line<'static> {
    let (nats_glyph, nats_col) = if state.nats_up {
        ("●", Color::Green)
    } else {
        ("○", Color::DarkGray)
    };
    let ctx_col = if state.ctx_pct >= 85 { Color::Red } else if state.ctx_pct >= 60 { Color::Yellow } else { Color::DarkGray };
    Line::from(vec![
        Span::styled(format!("wk {}", state.worker_count), Style::default().fg(Color::Gray)),
        Span::raw(" · nats "),
        Span::styled(nats_glyph, Style::default().fg(nats_col)),
        Span::raw(" · "),
        Span::styled(format!("ctx {}%", state.ctx_pct), Style::default().fg(ctx_col)),
    ])
}
```

- [ ] **Step 4: Run test, verify it passes**

Run: `cargo test -p entheai-viz brain`
Expected: all 6 tests pass. If the buffer-access API in the test differs, align it with `swarm.rs`.

- [ ] **Step 5: Clippy + commit**

Run: `cargo clippy -p entheai-viz -- -D warnings`

```bash
git add crates/viz/src/brain.rs
git commit -m "feat(viz): braille pseudo-3D brain render + wk/nats/ctx footer"
```

---

### Task 3: Config — `[viz] brain` / `brain_width` (`crates/config/src/lib.rs`)

**Files:**
- Modify: `crates/config/src/lib.rs` (`VizConfig` struct ~200, defaults ~215, `impl Default` ~231)
- Test: inline `#[cfg(test)]` in the same file (mirror existing viz-default tests)

- [ ] **Step 1: Write the failing tests** (add near the existing config tests)

```rust
    #[test]
    fn viz_brain_defaults_on() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.viz.brain);
        assert_eq!(cfg.viz.brain_width, 26);
    }

    #[test]
    fn viz_brain_overrides() {
        let cfg = Config::from_toml_str("[viz]\nbrain = false\nbrain_width = 30\n").unwrap();
        assert!(!cfg.viz.brain);
        assert_eq!(cfg.viz.brain_width, 30);
    }
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p entheai-config viz_brain`
Expected: compile error (no `brain` field).

- [ ] **Step 3: Add the fields + defaults**

In `struct VizConfig` (after `swarm_rows_cap`, ~line 212):

```rust
    #[serde(default = "default_viz_brain")]
    pub brain: bool,
    #[serde(default = "default_viz_brain_width")]
    pub brain_width: u16,
```

Add the default fns (next to `default_viz_swarm_rows_cap`, ~line 227):

```rust
fn default_viz_brain() -> bool { true }
fn default_viz_brain_width() -> u16 { 26 }
```

In `impl Default for VizConfig` (~231) add:

```rust
            brain: default_viz_brain(),
            brain_width: default_viz_brain_width(),
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p entheai-config viz_brain`
Expected: 2 pass.

- [ ] **Step 5: Commit**

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): [viz] brain + brain_width knobs"
```

---

### Task 4: `App` wiring + `/brain` toggle + visibility gate (`crates/tui/src/lib.rs`)

**Files:**
- Modify: `crates/tui/src/lib.rs` — `App` struct (~183), init (~390), a pure `show_brain` helper + const, `is_brain_command`/`handle_brain_command` (mirror `is_viz_command`/`handle_viz_command` at ~1118/1126), the slash-menu table (~1816) + help text (~1166), and the `Action::Submit` command dispatch (mirror the `/viz` guard around ~519/948).
- Test: inline `#[cfg(test)]` for `show_brain`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn brain_panel_visibility_gate() {
        assert!(show_brain(true, 100));
        assert!(!show_brain(false, 100));       // disabled
        assert!(!show_brain(true, 60));          // too narrow (< MIN_WIDTH_FOR_BRAIN)
    }
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-tui brain_panel_visibility_gate` → `show_brain` not found.

- [ ] **Step 3: Implement**

Add the const + helper (near `STATUS_ROWS`, ~826):

```rust
/// Minimum terminal width before the brain side panel is shown; below this it
/// auto-hides and the layout is byte-identical to the no-panel build.
const MIN_WIDTH_FOR_BRAIN: u16 = 72;

/// Pure visibility gate — the panel shows only when enabled and the terminal is
/// wide enough to spare `brain_width` columns without crowding the chat.
fn show_brain(enabled: bool, term_width: u16) -> bool {
    enabled && term_width >= MIN_WIDTH_FOR_BRAIN
}
```

Add `App` fields (in the struct, after `viz_swarm`, ~234):

```rust
    /// Always-on brain side panel model (faculties + fleet + readouts).
    brain: entheai_viz::BrainState,
    /// Whether the brain panel is shown (from `[viz] brain`, toggled by `/brain`).
    brain_enabled: bool,
```

Init (in the `App { ... }` literal, ~390-415, next to `swarm`/`viz_swarm`):

```rust
        brain: entheai_viz::BrainState::new(),
        brain_enabled: config.viz.brain,
```

Command predicate + handler (mirror `is_viz_command`/`handle_viz_command`, ~1118):

```rust
/// True when the submitted input is the local `/brain` toggle.
fn is_brain_command(text: &str) -> bool { text.trim() == "/brain" }

/// Toggle the always-on brain side panel in response to `/brain`.
fn handle_brain_command(app: &mut App) {
    app.brain_enabled = !app.brain_enabled;
    app.notice = Some(if app.brain_enabled { "brain panel on".into() } else { "brain panel off".into() });
}
```

Wire the command: wherever `/viz` is intercepted before being sent to the agent — the `Action::Submit(text) if is_viz_command(&text)` guard (~519) and the local-command predicate at ~948 (`|| is_viz_command(trimmed)`). Add a sibling `Action::Submit(text) if is_brain_command(&text) => { handle_brain_command(&mut app); }` arm and `|| is_brain_command(trimmed)` to the local-command set so it is never sent to the model.

Slash-menu entry (in the `SLASH` table, ~1816):

```rust
    ("/brain", "toggle the always-on brain side panel"),
```

Help text (~1166) — append `· /brain panel` to the command hints line.

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-tui brain_panel_visibility_gate` → passes.

- [ ] **Step 5: Build the whole crate** — `cargo build -p entheai-tui` → clean (fields unused until Task 5; add `#[allow(dead_code)]` only if the build *errors*, not for warnings — Task 5 consumes them next).

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): App brain state + /brain toggle + visibility gate"
```

---

### Task 5: Render split + panel draw (`crates/tui/src/lib.rs`)

**Files:**
- Modify: `crates/tui/src/lib.rs` — `render` (~1562): carve a right-hand column from `frame.area()`, feed the left rect into both existing `ViewMode` branches, draw the brain panel into the right rect.

- [ ] **Step 1: Carve the split** — in `render`, replace the single `let area = frame.area();` (~1571) with:

```rust
    let full = frame.area();
    let show = show_brain(app.brain_enabled, full.width);
    let (area, brain_area) = if show {
        let bw = config_brain_width; // see Step 2 for how brain_width reaches render
        let [left, right] = Layout::horizontal([Constraint::Min(1), Constraint::Length(bw)]).areas(full);
        (left, Some(right))
    } else {
        (full, None)
    };
```

Every existing use of `area` below (both the `ViewMode::Swarm` branch's `Layout::vertical(...).areas(area)` at ~1577 and the chat branch's at ~1601) now operates on the left sub-rect unchanged.

- [ ] **Step 2: Thread `brain_width` into `render`**

`render`'s signature (~1562) does not currently receive config. `brain_width` is a `u16` — pass it as a new parameter. At the call site (`render(frame, &app, lines, scroll, plan_rows, swarm_rows, &env_line)`, ~462) add `config.viz.brain_width` as the trailing arg, and add `brain_width: u16` to the `render` signature. (`config` is in scope at the call site.)

- [ ] **Step 3: Draw the panel** — at the end of `render`, before the final `render_slash_menu`/return, add:

```rust
    if let Some(ba) = brain_area {
        let block = Block::default().borders(Borders::ALL).title(" brain ");
        let inner = block.inner(ba);
        frame.render_widget(block, ba);
        entheai_viz::brain::render(&app.brain, inner, frame.buffer_mut(), ratatui::symbols::Marker::Braille);
    }
```

Place this so it runs for **both** views (put it after the `ViewMode::Swarm` early-return is restructured, OR draw it in each branch). Simplest: draw the brain panel first (it uses `brain_area`, independent of the view), then let the view branches render into `area`. Since the `Swarm` branch `return`s early (~1598), draw the brain panel *before* the `if app.view == ViewMode::Swarm` check so it appears in both views.

- [ ] **Step 4: Build + visual check**

Run: `cargo build -p entheai` (the bin re-exports the TUI). Expected: clean.
Then a real launch (Ghostty tier is computer-use "click" — no synthetic typing): rebuild release and relaunch `--app`, confirm the ` brain ` panel appears on the right with a rotating M/T/C + core and the `wk 0 · nats ○ · ctx N%` footer. (Fleet/nats stay 0/○ until Task 7; faculties stay dim until Task 6.)

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): horizontal split + brain side panel draw in both views"
```

---

### Task 6: Event taps + free-running rotation (`crates/tui/src/lib.rs`)

**Files:**
- Modify: `crates/tui/src/lib.rs` — the `events_rx` arms (`Token` ~680, `Thinking` ~674, `ToolStarted` ~696, `ToolFinished` ~710), the `ticker` arm (~803), and the per-draw ctx update (~near 441/462).

- [ ] **Step 1: Flare faculties from existing arms**

- In the `AgentEvent::Thinking`/`Token` arms (~674/680) add: `app.brain.flare(entheai_viz::FacultyKind::Model);`
- In the `AgentEvent::ToolStarted`/`ToolFinished` arms (~696/710) add: `app.brain.flare(entheai_viz::FacultyKind::Tools);`

- [ ] **Step 2: Free-running rotation + repaint** — in the `ticker` arm (~803), currently gated by `Status::Working`. Add, unconditionally (outside the `Working` gate):

```rust
            if show_brain(app.brain_enabled, last_known_width) {
                app.brain.tick();
                dirty = true;
            }
```

`last_known_width`: the loop must know the terminal width off the render path. Use `crossterm::terminal::size()` (cheap) cached into a loop local updated on the `Resize` input event, or call `size()` in the ticker arm. Prefer caching a `term_width: u16` local, initialized before the loop from `crossterm::terminal::size()` and refreshed in the crossterm `Event::Resize(w, _)` arm (~468 input handling).

- [ ] **Step 3: Update ctx% each draw** — where `plan_rows`/`swarm_rows` are computed before `terminal.draw` (~441-462), add:

```rust
            let ctx_pct = {
                let cur = est_context_tokens(&app);
                let max = max_context_window(&app.model_label).max(1);
                ((cur.saturating_mul(100) / max).min(999)) as u16
            };
            app.brain.set_ctx_pct(ctx_pct);
```

(Compute into a local first, then `set_ctx_pct` — keeps the borrow of `&app` and the `&mut app.brain` disjoint in time.)

- [ ] **Step 4: Build + visual check**

Run: `cargo build -p entheai`. Relaunch. Confirm: the M and T faculty nodes brighten when the agent is generating / running a tool and fade afterward; the graph rotates continuously even when idle; `ctx %` in the footer tracks the real context fill.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): flare model/tools faculties from agent events + free-running rotation + live ctx%"
```

---

### Task 7: Throttled fleet poll → fleet nodes + nats indicator (`crates/tui/src/lib.rs`)

**Files:**
- Modify: `crates/tui/src/lib.rs` — add a fleet-poll `interval` to the `select!` loop; seed `nats_up` at startup from `bus`/`fleet_fed`.

- [ ] **Step 1: Seed nats_up at startup** — after `fleet_fed`/`bus` are established (~344/363), before/at the start of the loop:

```rust
    app.brain.set_nats(bus.is_some() || fleet_fed.is_some());
```

- [ ] **Step 2: Add the poll interval** — near the `ticker` definition (~428):

```rust
    let mut fleet_poll = tokio::time::interval(Duration::from_millis(1500));
    fleet_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
```

- [ ] **Step 3: Add the poll arm** to the `select!` (sibling to `ticker`, ~803):

```rust
            _ = fleet_poll.tick() => {
                if let Some(fed) = &fleet_fed {
                    let workers = fed.list_workers(Duration::from_millis(600)).await;
                    let tuples: Vec<(String, bool)> = workers.iter()
                        .map(|w| (w.node_id.clone(), matches!(w.state, entheai_federation::WorkerState::Working { .. })))
                        .collect();
                    app.brain.set_fleet(&tuples);
                    app.brain.set_nats(true); // responded → NATS reachable
                    dirty = true;
                } else {
                    // federation off: ensure fleet is empty, nats reflects the bus only
                    if !app.brain.fleet.is_empty() { app.brain.set_fleet(&[]); dirty = true; }
                }
            }
```

Note: `list_workers(600ms).await` briefly yields the loop — acceptable at a 1.5 s cadence (same call `/fleet` already makes). Do **not** shorten the interval below ~1 s.

- [ ] **Step 4: Build + clippy**

Run: `cargo build -p entheai && cargo clippy -p entheai-tui -- -D warnings`. Expected: clean.

- [ ] **Step 5: Live E2E** — with a worker serving (or against the dev-cx53 fleet from [[dev-cx53-sandbox]]), confirm the outer-ring fleet nodes appear (green when working), `wk N` tracks the count, and `nats ●` shows when connected / `○` when federation is off. Rebuild release + relaunch `--app`.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): throttled fleet poll → brain fleet nodes + nats ●/○ indicator"
```

---

### Task 8: Docs — `entheai.toml` + CHANGELOG

**Files:**
- Modify: `entheai.toml` (document the `[viz] brain` / `brain_width` knobs)
- Modify: `CHANGELOG.md` (Unreleased → Added)

- [ ] **Step 1: `entheai.toml`** — add under a `[viz]` section (create it if absent; it's currently all-default):

```toml
# ── TUI visualization ─────────────────────────────────────────────────────────
# [viz]
# brain = true         # always-on side panel: a rotating faculties+fleet graph
#                      # with `wk N · nats ●/○ · ctx %`. Toggle live with /brain.
# brain_width = 26     # panel width in columns (auto-hides on narrow terminals)
```

- [ ] **Step 2: `CHANGELOG.md`** — add to `## [Unreleased]` → `### Added`:

```markdown
- **Brain panel (TUI).** An always-on compact side panel beside the chat: a slowly rotating braille pseudo-3D node graph of the agent's faculties (model · tools · context) and the remote fleet, with a live `wk N · nats ●/○ · ctx %` footer. Faculties flare on token generation / tool calls and decay; the fleet ring and NATS indicator come from a throttled 1.5 s presence poll. Toggle with `/brain`, config `[viz] brain` / `brain_width`; auto-hides on narrow terminals. (Slice B — a kitty-graphics true-3D upgrade behind `graphics_capable()` — is a planned follow-on.)
```

- [ ] **Step 3: Commit**

```bash
git add entheai.toml CHANGELOG.md
git commit -m "docs: document [viz] brain panel + CHANGELOG entry"
```

---

## Final verification (after all tasks)

- [ ] `cargo test -p entheai-viz -p entheai-config -p entheai-tui` — all green.
- [ ] `cargo clippy --workspace -- -D warnings` — clean.
- [ ] `cargo build --no-default-features -p entheai` — headless build still compiles (brain panel is pure ratatui, no GUI deps).
- [ ] Release rebuild + `--app` relaunch: panel renders, rotates, faculties flare on activity, footer readouts live.
- [ ] Dispatch a final holistic code-reviewer subagent over the whole Slice-A diff.
