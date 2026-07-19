# viz Slice 1 — the Swarm — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new standalone `crates/viz` crate that turns the live `FanoutEvent` stream into an animated ratatui swarm graph, wired into the TUI as an inline pane during fan-out plus a full-screen `Ctrl-V` viz-mode.

**Architecture:** `crates/viz` is a pure, orchestrator-free crate: a deterministic `SwarmModel` state machine (semantic mutators, no clock, no I/O), a `swarm::render` that draws the model onto a ratatui `Canvas` at any size, and a `term::graphics_capable()` env probe. The TUI owns a `SwarmModel`, maps each `FanoutEvent` to a mutator (the same stream that feeds the plan pane), and renders it inline (collapses to 0 rows when idle) or full-screen.

**Tech Stack:** Rust, `ratatui` 0.29 (`Canvas`, `TestBackend`), `crossterm` 0.28. No new third-party deps.

---

> ## Scope & the multi-session hazard — READ FIRST
>
> - **Scope = Slice 1 (the swarm) only.** The wgpu→Kitty **shader** (Slice 2) is a separate spec/plan. Do not build it here.
> - **Tasks 1–5 are collision-free** — a brand-new `crates/viz` crate + a `crates/config` addition. Do these now regardless of `main`'s state.
> - **Task 6 (TUI integration) is GATED.** `crates/tui/src/lib.rs` is currently being edited by a concurrent fan-out/workers session and shared `main` does **not** compile (its `run_fanout` now takes a 5th `pool` arg with a broken TUI call site). **Do not start Task 6 until `cargo build -p entheai-tui` is green on `main` again.** When you do, re-read the current `crates/tui/src/lib.rs` — line numbers below are from commit-era `~1439`-line state and will have moved.
> - **Every commit:** scoped, explicit-pathspec (`git commit -m "..." -- <paths>`) to dodge the concurrent auto-stager (`git add -u` on Bash), then push immediately; rebase on non-FF (`git pull --rebase origin main`; if `.repowise/wiki.db` blocks, `git stash push -- .repowise/wiki.db` → rebase → `git stash pop`). New files need `git add <path>` first (pathspec-commit alone won't add an untracked file). Never `git add -A`/`.`, never `git reset --hard`.

## File structure

| File | Responsibility |
|---|---|
| `crates/viz/Cargo.toml` | new crate manifest; deps: `ratatui` (workspace). **No** orchestrator/tokio dep. |
| `crates/viz/src/lib.rs` | module decls + re-exports (`SwarmModel`, `NodeStatus`, `SwarmNode`, `Phase`). |
| `crates/viz/src/model.rs` | the pure `SwarmModel` state machine + mutators + queries. |
| `crates/viz/src/term.rs` | `graphics_capable()` terminal probe. |
| `crates/viz/src/swarm.rs` | `render(model, area, buf, marker, frame)` → ratatui `Canvas`. |
| `Cargo.toml` (workspace) | add `crates/viz` to `members`. |
| `crates/config/src/lib.rs` | `VizConfig { swarm: bool }` (default true) + `Config.viz`. |
| `crates/tui/Cargo.toml` | add `entheai-viz` dep. *(Task 6)* |
| `crates/tui/src/lib.rs` | `swarm: SwarmModel` + `view: ViewMode`; feed `FanoutEvent`→mutators; inline pane; `Ctrl-V`/`/viz` toggle; render. *(Task 6, gated)* |

---

## Task 1: `crates/viz` scaffold + `SwarmModel::decompose`

**Files:**
- Create: `crates/viz/Cargo.toml`, `crates/viz/src/lib.rs`, `crates/viz/src/model.rs`
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Create the crate manifest**

`crates/viz/Cargo.toml`:
```toml
[package]
name = "entheai-viz"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
ratatui = { workspace = true }
```

- [ ] **Step 2: Add the crate to the workspace**

In the root `Cargo.toml`, add `"crates/viz"` to the `members` array (append before `"bin/entheai"`). The line currently ends `..., "crates/skills", "bin/entheai", "bin/entheai-worker"]`; make it `..., "crates/skills", "crates/viz", "bin/entheai", "bin/entheai-worker"]`.

- [ ] **Step 3: Write the failing test** in `crates/viz/src/model.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompose_seeds_pending_nodes() {
        let mut m = SwarmModel::new();
        m.decompose(&[("coder".into(), "add retry".into()), ("test".into(), "cover it".into())]);
        assert_eq!(m.nodes.len(), 2);
        assert!(m.nodes.iter().all(|n| n.status == NodeStatus::Pending));
        assert_eq!(m.nodes[0].role, "coder");
        assert_eq!(m.nodes[1].index, 1);
        assert!(m.is_active(), "fan-out is active after decompose");
    }
}
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p entheai-viz decompose_seeds_pending_nodes`
Expected: FAIL to compile — `cannot find type SwarmModel`.

- [ ] **Step 5: Implement the model core** in `crates/viz/src/model.rs` (above the `#[cfg(test)]` block):

```rust
//! The swarm state machine: a deterministic model of a fan-out run, folded from
//! semantic mutator calls (the TUI maps `FanoutEvent`s onto these). No clock, no
//! I/O, no orchestrator dependency — trivially unit-testable.

/// Lifecycle status of one sub-agent node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Pending,
    Running,
    Done,
    Failed,
}

/// One sub-agent in the swarm.
#[derive(Debug, Clone)]
pub struct SwarmNode {
    pub index: usize,
    pub role: String,
    pub task: String,
    pub status: NodeStatus,
    pub committed: bool,
}

/// Overall phase of the fan-out run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Fanning,
    Integrating,
    Done,
}

/// The full swarm model. `Default` = an idle, empty swarm.
#[derive(Debug, Clone, Default)]
pub struct SwarmModel {
    pub nodes: Vec<SwarmNode>,
    pub phase: Phase,
    pub integrating_branches: usize,
    pub merged: usize,
    pub conflicted: usize,
    pub integration_branch: Option<String>,
}

impl SwarmModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset to idle/empty, then seed one `Pending` node per sub-task.
    pub fn decompose(&mut self, tasks: &[(String, String)]) {
        *self = Self::default();
        self.phase = Phase::Fanning;
        self.nodes = tasks
            .iter()
            .enumerate()
            .map(|(index, (role, task))| SwarmNode {
                index,
                role: role.clone(),
                task: task.clone(),
                status: NodeStatus::Pending,
                committed: false,
            })
            .collect();
    }

    /// A fan-out is on screen (fanning out or integrating).
    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Fanning | Phase::Integrating)
    }
}
```

- [ ] **Step 6: Run it to verify it passes**

Run: `cargo test -p entheai-viz decompose_seeds_pending_nodes`
Expected: PASS.

- [ ] **Step 7: Add the module declarations** to `crates/viz/src/lib.rs`:

```rust
//! entheai visualization — Slice 1: the fan-out swarm.
//!
//! A pure, terminal-agnostic model of a fan-out run plus a ratatui renderer.
//! Fed by the TUI from the existing `FanoutEvent` stream (mapped to mutators).

pub mod model;
pub mod swarm;
pub mod term;

pub use model::{NodeStatus, Phase, SwarmModel, SwarmNode};
```

Because `swarm` and `term` don't exist yet, create empty stubs so the crate compiles now: `crates/viz/src/swarm.rs` with `//! placeholder — implemented in Task 4` and `crates/viz/src/term.rs` with `//! placeholder — implemented in Task 3`. (They gain real content in later tasks.)

- [ ] **Step 8: Commit**

Run: `cargo test -p entheai-viz` → PASS. `cargo clippy -p entheai-viz -- -D warnings` → clean. `cargo fmt -p entheai-viz`.

```bash
git add crates/viz/Cargo.toml crates/viz/src/lib.rs crates/viz/src/model.rs crates/viz/src/swarm.rs crates/viz/src/term.rs Cargo.toml Cargo.lock
git commit -m "feat(viz): new crate — SwarmModel + decompose (Slice 1 scaffold)" -- crates/viz/Cargo.toml crates/viz/src/lib.rs crates/viz/src/model.rs crates/viz/src/swarm.rs crates/viz/src/term.rs Cargo.toml Cargo.lock
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 2: `SwarmModel` mutators — started / finished / integrating / done

**Files:**
- Modify: `crates/viz/src/model.rs`

- [ ] **Step 1: Write the failing tests** (add to `mod tests`):

```rust
#[test]
fn coder_started_marks_running() {
    let mut m = SwarmModel::new();
    m.decompose(&[("coder".into(), "t".into())]);
    m.coder_started(0, "coder", "t");
    assert_eq!(m.nodes[0].status, NodeStatus::Running);
    assert_eq!(m.running(), 1);
}

#[test]
fn coder_finished_marks_done_or_failed_from_status() {
    let mut m = SwarmModel::new();
    m.decompose(&[("a".into(), "t".into()), ("b".into(), "t".into())]);
    m.coder_finished(0, true, "verified");
    m.coder_finished(1, false, "verify failed");
    assert_eq!(m.nodes[0].status, NodeStatus::Done);
    assert!(m.nodes[0].committed);
    assert_eq!(m.nodes[1].status, NodeStatus::Failed);
    assert_eq!(m.done_count(), 1);
    assert_eq!(m.failed_count(), 1);
}

#[test]
fn started_for_unknown_index_adds_a_node() {
    // Defensive: a CoderStarted without a preceding Decomposed still shows up.
    let mut m = SwarmModel::new();
    m.coder_started(3, "coder", "t");
    assert_eq!(m.nodes.len(), 1);
    assert_eq!(m.nodes[0].index, 3);
    assert_eq!(m.nodes[0].status, NodeStatus::Running);
}

#[test]
fn integrating_then_done_sets_phase_and_totals() {
    let mut m = SwarmModel::new();
    m.decompose(&[("a".into(), "t".into())]);
    m.integrating(1);
    assert_eq!(m.phase, Phase::Integrating);
    assert!(m.is_active());
    m.done(Some("entheai/fanout-x".into()), 1, 0);
    assert_eq!(m.phase, Phase::Done);
    assert!(!m.is_active(), "done runs are no longer active");
    assert_eq!(m.merged, 1);
    assert_eq!(m.integration_branch.as_deref(), Some("entheai/fanout-x"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p entheai-viz coder_started_marks_running`
Expected: FAIL — `no method named coder_started`.

- [ ] **Step 3: Implement the mutators + queries** (add to `impl SwarmModel`):

```rust
    /// Mark node `index` as running. If it wasn't seeded (a `CoderStarted`
    /// without a preceding `Decomposed`), add it — the swarm should never drop
    /// an agent that actually ran.
    pub fn coder_started(&mut self, index: usize, role: &str, task: &str) {
        match self.nodes.iter_mut().find(|n| n.index == index) {
            Some(node) => node.status = NodeStatus::Running,
            None => self.nodes.push(SwarmNode {
                index,
                role: role.to_string(),
                task: task.to_string(),
                status: NodeStatus::Running,
                committed: false,
            }),
        }
        if self.phase == Phase::Idle {
            self.phase = Phase::Fanning;
        }
    }

    /// Mark node `index` finished. `status` is the fan-out's human summary
    /// (e.g. "verified", "verify failed", "no changes"); a summary containing
    /// "fail" → `Failed`, otherwise `Done`.
    pub fn coder_finished(&mut self, index: usize, committed: bool, status: &str) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.index == index) {
            node.committed = committed;
            node.status = if status.to_ascii_lowercase().contains("fail") {
                NodeStatus::Failed
            } else {
                NodeStatus::Done
            };
        }
    }

    /// Enter the integrate phase.
    pub fn integrating(&mut self, branches: usize) {
        self.integrating_branches = branches;
        self.phase = Phase::Integrating;
    }

    /// Fan-out finished — record the integration outcome.
    pub fn done(&mut self, integration_branch: Option<String>, merged: usize, conflicted: usize) {
        self.phase = Phase::Done;
        self.integration_branch = integration_branch;
        self.merged = merged;
        self.conflicted = conflicted;
    }

    pub fn running(&self) -> usize {
        self.nodes.iter().filter(|n| n.status == NodeStatus::Running).count()
    }
    pub fn done_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.status == NodeStatus::Done).count()
    }
    pub fn failed_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.status == NodeStatus::Failed).count()
    }
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p entheai-viz` → Expected: all pass (5 tests).

- [ ] **Step 5: Commit**

`cargo clippy -p entheai-viz -- -D warnings` → clean. `cargo fmt -p entheai-viz`.

```bash
git add crates/viz/src/model.rs
git commit -m "feat(viz): SwarmModel mutators (started/finished/integrating/done) + counts" -- crates/viz/src/model.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 3: `viz::term::graphics_capable()`

**Files:**
- Modify: `crates/viz/src/term.rs`

- [ ] **Step 1: Write the failing test** (in `crates/viz/src/term.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_kitty_graphics_terminals() {
        assert!(is_graphics_term(Some("ghostty"), None));
        assert!(is_graphics_term(Some("WezTerm"), None));
        assert!(is_graphics_term(None, Some("xterm-kitty")));
        assert!(!is_graphics_term(Some("Apple_Terminal"), Some("xterm-256color")));
        assert!(!is_graphics_term(None, None));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-viz detects_kitty_graphics_terminals`
Expected: FAIL — `cannot find function is_graphics_term`.

- [ ] **Step 3: Implement** (replace the placeholder in `crates/viz/src/term.rs`):

```rust
//! Terminal capability probe for the Kitty graphics protocol (used by Slice 2's
//! shader; exposed now so the TUI can label/gate viz features).

/// Pure decision from the two relevant env values — testable without touching
/// the real environment.
fn is_graphics_term(term_program: Option<&str>, term: Option<&str>) -> bool {
    let tp = term_program.unwrap_or("").to_ascii_lowercase();
    if tp.contains("ghostty") || tp.contains("wezterm") || tp.contains("kitty") {
        return true;
    }
    let t = term.unwrap_or("").to_ascii_lowercase();
    t.contains("kitty")
}

/// True when the current terminal supports the Kitty graphics protocol
/// (Ghostty / Kitty / WezTerm). Reads `$TERM_PROGRAM` and `$TERM`.
pub fn graphics_capable() -> bool {
    is_graphics_term(
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-viz detects_kitty_graphics_terminals`
Expected: PASS.

- [ ] **Step 5: Commit**

`cargo clippy -p entheai-viz -- -D warnings` → clean. `cargo fmt -p entheai-viz`.

```bash
git add crates/viz/src/term.rs
git commit -m "feat(viz): term::graphics_capable() Kitty-protocol probe" -- crates/viz/src/term.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 4: `viz::swarm::render` — ratatui Canvas

**Files:**
- Modify: `crates/viz/src/swarm.rs`

- [ ] **Step 1: Write the failing tests** (in `crates/viz/src/swarm.rs`):

The test renders into a ratatui `Buffer` and scans it as text — robust against exact Canvas cell positions.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::SwarmModel;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::symbols::Marker;

    fn buf_text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn empty_model_draws_no_status_glyphs() {
        let m = SwarmModel::new();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        render(&m, area, &mut buf, Marker::Braille, 0);
        let text = buf_text(&buf);
        assert!(!text.contains('✓') && !text.contains('◐') && !text.contains('◻'));
    }

    #[test]
    fn nodes_render_status_glyphs() {
        let mut m = SwarmModel::new();
        m.decompose(&[("coder".into(), "t".into()), ("test".into(), "t".into())]);
        m.coder_started(0, "coder", "t");
        m.coder_finished(1, true, "verified");
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        render(&m, area, &mut buf, Marker::Braille, 1);
        let text = buf_text(&buf);
        assert!(text.contains('◐'), "running node glyph present");
        assert!(text.contains('✓'), "done node glyph present");
        assert!(text.contains('o') || text.contains('c'), "a role label rendered");
    }

    #[test]
    fn tiny_and_large_areas_do_not_panic() {
        let mut m = SwarmModel::new();
        m.decompose(&[("a".into(), "t".into())]);
        for (w, h) in [(10u16, 3u16), (120, 40)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            render(&m, area, &mut buf, Marker::Braille, 7);
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-viz -- swarm::`
Expected: FAIL — `cannot find function render`.

- [ ] **Step 3: Implement `render`** (replace the placeholder in `crates/viz/src/swarm.rs`):

```rust
//! Draw a [`SwarmModel`] onto a ratatui `Canvas`. The same function serves the
//! small inline pane and the full-screen viz-mode — only `area` changes.

use ratatui::layout::Rect;
use ratatui::prelude::Buffer;
use ratatui::style::Color;
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Points};
use ratatui::widgets::Widget;

use crate::model::{NodeStatus, SwarmModel};

/// Glyph for a node status.
fn glyph(status: NodeStatus, frame: u64) -> char {
    match status {
        NodeStatus::Pending => '◻',
        // pulse: alternate the running glyph by frame so it visibly breathes.
        NodeStatus::Running => {
            if frame % 2 == 0 {
                '◐'
            } else {
                '◑'
            }
        }
        NodeStatus::Done => '✓',
        NodeStatus::Failed => '✗',
    }
}

fn status_color(status: NodeStatus) -> Color {
    match status {
        NodeStatus::Pending => Color::DarkGray,
        NodeStatus::Running => Color::Cyan,
        NodeStatus::Done => Color::Green,
        NodeStatus::Failed => Color::Red,
    }
}

/// Render the swarm. `marker` picks the Canvas resolution (Braille inline,
/// HalfBlock for a chunkier full view); `frame` drives the running pulse.
pub fn render(model: &SwarmModel, area: Rect, buf: &mut Buffer, marker: Marker, frame: u64) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let w = f64::from(area.width);
    let h = f64::from(area.height);

    // Orchestrator at top-center; nodes fanned across a lower row.
    let orch = (w / 2.0, h * 0.85);
    let node_xy = |i: usize| -> (f64, f64) {
        let x = if model.nodes.len() <= 1 {
            w / 2.0
        } else {
            w * (0.10 + 0.80 * (i as f64) / ((model.nodes.len() - 1) as f64))
        };
        (x, h * 0.20)
    };

    let nodes = model.nodes.clone();
    Canvas::default()
        .marker(marker)
        .x_bounds([0.0, w])
        .y_bounds([0.0, h])
        .paint(move |ctx| {
            // Edges orchestrator → each node.
            for (i, node) in nodes.iter().enumerate() {
                let (nx, ny) = node_xy(i);
                ctx.draw(&CanvasLine {
                    x1: orch.0,
                    y1: orch.1,
                    x2: nx,
                    y2: ny,
                    color: Color::DarkGray,
                });
            }
            ctx.layer();
            // Orchestrator node.
            ctx.draw(&Points {
                coords: &[orch],
                color: Color::Blue,
            });
            ctx.print(orch.0, orch.1, "orch");
            // Sub-agent nodes: a colored point, a status glyph, a short role label.
            for (i, node) in nodes.iter().enumerate() {
                let (nx, ny) = node_xy(i);
                ctx.draw(&Points {
                    coords: &[(nx, ny)],
                    color: status_color(node.status),
                });
                let label: String = node.role.chars().take(6).collect();
                ctx.print(nx, ny, format!("{} {label}", glyph(node.status, frame)));
            }
        })
        .render(area, buf);
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-viz -- swarm::`
Expected: PASS (3 tests). If `ctx.print` requires an owned `String` vs `&str` mismatch, wrap literals with `.to_string()`; if `buf.content()` is private in your ratatui patch version, use `(0..area.area()).map(|i| buf.content[i as usize].symbol())` — adjust to the 0.29 API and note it.

- [ ] **Step 5: Commit**

`cargo clippy -p entheai-viz -- -D warnings` → clean. `cargo fmt -p entheai-viz`.

```bash
git add crates/viz/src/swarm.rs
git commit -m "feat(viz): swarm::render — ratatui Canvas graph (nodes/edges/glyphs/pulse)" -- crates/viz/src/swarm.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 5: `[viz]` config

**Files:**
- Modify: `crates/config/src/lib.rs`

- [ ] **Step 1: Write the failing test** (add to the `#[cfg(test)] mod tests` in `crates/config/src/lib.rs`):

```rust
#[test]
fn viz_config_defaults() {
    let cfg = Config::from_toml_str("").unwrap();
    assert!(cfg.viz.swarm, "the swarm is on by default");
}

#[test]
fn viz_swarm_can_be_disabled() {
    let cfg = Config::from_toml_str("[viz]\nswarm = false\n").unwrap();
    assert!(!cfg.viz.swarm);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-config viz_config_defaults`
Expected: FAIL — no field `viz` on `Config`.

- [ ] **Step 3: Implement.** Add a field to `struct Config` (next to the other `#[serde(default)]` sections like `memory`):

```rust
    #[serde(default)]
    pub viz: VizConfig,
```

Add the type + default near the other config structs (e.g. below `MemoryConfig`):

```rust
/// Visualization settings (viz pillar).
#[derive(Debug, Clone, Deserialize)]
pub struct VizConfig {
    /// Show the live fan-out swarm (inline pane + Ctrl-V full view).
    #[serde(default = "default_viz_swarm")]
    pub swarm: bool,
}

fn default_viz_swarm() -> bool {
    true
}

impl Default for VizConfig {
    fn default() -> Self {
        Self {
            swarm: default_viz_swarm(),
        }
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-config viz_config_defaults viz_swarm_can_be_disabled`
Expected: PASS.

- [ ] **Step 5: Commit**

`cargo test -p entheai-config` → all pass. `cargo clippy -p entheai-config -- -D warnings` → clean. `cargo fmt -p entheai-config`.

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): [viz] swarm on-by-default" -- crates/config/src/lib.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 6: TUI integration — inline pane + `Ctrl-V` viz-mode  ⛔ GATED

> **DO NOT START until `cargo build -p entheai-tui` is green on `main`** (the concurrent fan-out session must land its `run_fanout(pool)` call-site fix first). Then **re-read `crates/tui/src/lib.rs`** — the anchors below are from the ~1439-line state and will have shifted. Verify each referenced item still exists before editing.

**Files:**
- Modify: `crates/tui/Cargo.toml`, `crates/tui/src/lib.rs`

**Anchors in the current TUI (verify before editing):**
- `struct App` (~line 183) with fields incl. `fanout: bool`, `plan: Vec<entheai_tools::todo::TodoItem>`.
- `plan_rows_for(plan_len) -> u16` (~646, `PLAN_ROWS_CAP = 8`, returns 0 when empty).
- The layout math (~360): `plan_rows = plan_rows_for(app.plan.len()); …saturating_sub(STATUS_ROWS + PROGRESS_ROWS + INPUT_ROWS + plan_rows)`.
- `render(frame, &app, lines, scroll, plan_rows)` (~376).
- The `FanoutEvent` match (~552–613) that already updates `app.plan` for `Decomposed/CoderStarted/CoderFinished/Integrating/Done`.
- Key handling with `KeyCode::Char('p'|'n'|'c') if CONTROL` (~690–697).

- [ ] **Step 1: Add the dependency**

`crates/tui/Cargo.toml` `[dependencies]`: add `entheai-viz = { path = "../viz" }` (matching the sibling-path style already used for `entheai-orchestrator`).

- [ ] **Step 2: Add swarm + view state to `App`**

In `struct App`, add:
```rust
    /// Live fan-out swarm model (fed from the same FanoutEvent stream as `plan`).
    swarm: entheai_viz::SwarmModel,
    /// Which main view is showing.
    view: ViewMode,
    /// Whether the swarm viz is enabled (from `[viz] swarm`).
    viz_swarm: bool,
```
Add the enum near `struct App`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Chat,
    Swarm,
}
```
Initialize in the `App { … }` constructor (~337): `swarm: entheai_viz::SwarmModel::new(), view: ViewMode::Chat, viz_swarm: config.viz.swarm,`. (`config` is in scope where `App` is built; confirm the field name.)

- [ ] **Step 3: Feed FanoutEvents into the swarm**

In the `FanoutEvent` match (~552), add a mutator call **alongside** the existing `app.plan` updates (do not remove the plan handling):
```rust
    Some(entheai_orchestrator::FanoutEvent::Decomposed { tasks }) => {
        app.swarm.decompose(tasks);
        // …existing app.plan seeding stays…
    }
    Some(entheai_orchestrator::FanoutEvent::CoderStarted { index, role, task }) => {
        app.swarm.coder_started(*index, role, task);
        // …existing…
    }
    Some(entheai_orchestrator::FanoutEvent::CoderFinished { index, committed, status }) => {
        app.swarm.coder_finished(*index, *committed, status);
        // …existing…
    }
    Some(entheai_orchestrator::FanoutEvent::Integrating { branches }) => {
        app.swarm.integrating(*branches);
        // …existing…
    }
    Some(entheai_orchestrator::FanoutEvent::Done { integration_branch, merged, conflicted }) => {
        app.swarm.done(integration_branch.clone(), *merged, *conflicted);
        // …existing (incl. `fanout_rx = None`)…
    }
```
Match the existing binding style (the current arms bind by value/ref — mirror them; the calls above assume `index/committed/merged` are `&usize`/`&bool` from the matched ref, hence the `*`; adjust if the arm binds by value).

- [ ] **Step 4: Add a swarm-rows helper**

Add next to `plan_rows_for`:
```rust
const SWARM_PANE_CAP: u16 = 8;

/// Inline swarm-pane height: 0 unless enabled AND a fan-out is active; otherwise
/// `min(nodes + 2 border, SWARM_PANE_CAP)`. Zero → the pane collapses.
fn swarm_rows_for(enabled: bool, model: &entheai_viz::SwarmModel) -> u16 {
    if !enabled || !model.is_active() || model.nodes.is_empty() {
        0
    } else {
        ((model.nodes.len() as u16) + 2).min(SWARM_PANE_CAP)
    }
}
```

- [ ] **Step 5: Reserve the inline rows in the layout**

At the layout math (~360), reserve swarm rows (Chat view only) alongside plan rows:
```rust
    let plan_rows = plan_rows_for(app.plan.len());
    let swarm_rows = if app.view == ViewMode::Chat {
        swarm_rows_for(app.viz_swarm, &app.swarm)
    } else {
        0
    };
    let history_height = size.height
        .saturating_sub(STATUS_ROWS + PROGRESS_ROWS + INPUT_ROWS + plan_rows + swarm_rows);
```
Pass `swarm_rows` (and the frame counter already used for animation) into `render(frame, &app, lines, scroll, plan_rows, swarm_rows)` — update the `render` signature accordingly.

- [ ] **Step 6: Render inline pane / full view**

In `render`, in **Swarm** view draw the full-area swarm; in **Chat** view draw the inline pane when `swarm_rows > 0`. Using the existing region-splitting style (mirror how the plan pane is drawn), add:
```rust
    // Full viz-mode: the main content area IS the swarm.
    if app.view == ViewMode::Swarm {
        let block = Block::bordered().title(" swarm — Ctrl-V to exit ");
        let inner = block.inner(main_area);
        block.render(main_area, frame.buffer_mut());
        entheai_viz::swarm::render(&app.swarm, inner, frame.buffer_mut(), ratatui::symbols::Marker::HalfBlock, app.frame);
        // …still draw status + input rows as usual…
        return;
    }
    // Inline pane (Chat view), when reserved.
    if swarm_rows > 0 {
        let block = Block::bordered().title(" swarm ");
        let inner = block.inner(swarm_area);
        block.render(swarm_area, frame.buffer_mut());
        entheai_viz::swarm::render(&app.swarm, inner, frame.buffer_mut(), ratatui::symbols::Marker::Braille, app.frame);
    }
```
(Use the real frame-counter field name — the TUI-transparency pillar added one for the spinner/verb rotation; reuse it as `frame`. If none is threaded into `render`, pass the existing tick/`verb_idx`-style counter.)

- [ ] **Step 7: Toggle with `Ctrl-V` and `/viz`**

In the key handler, next to the `Ctrl-P/N/C` arms (~690):
```rust
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.view = match app.view {
                ViewMode::Chat => ViewMode::Swarm,
                ViewMode::Swarm => ViewMode::Chat,
            };
        }
```
And where submitted input is handled (the `Enter` path that inspects `text` for slash-commands — find the existing `/`-command handling, e.g. `/workers`), add a `/viz` case that toggles `app.view` the same way and does NOT dispatch to the agent.

- [ ] **Step 8: Build + targeted checks**

Run: `cargo build -p entheai-tui -p entheai` → compiles.
Run: `cargo test -p entheai-tui` → existing tests pass.
Add one behavioral test (TUI already has render/state tests — mirror them) asserting `swarm_rows_for`:
```rust
#[test]
fn swarm_pane_collapses_when_idle() {
    let m = entheai_viz::SwarmModel::new(); // Idle, empty
    assert_eq!(swarm_rows_for(true, &m), 0);
    let mut active = entheai_viz::SwarmModel::new();
    active.decompose(&[("a".into(), "t".into())]);
    assert_eq!(swarm_rows_for(true, &active), 3); // 1 node + 2 border
    assert_eq!(swarm_rows_for(false, &active), 0); // disabled → collapsed
}
```

- [ ] **Step 9: Full gate + commit**

Run: `./scripts/check.sh` (or, if other crates are mid-flight, `cargo build -p entheai-tui -p entheai && cargo clippy -p entheai-tui -- -D warnings && cargo fmt -p entheai-tui --check && cargo test -p entheai-tui`).

```bash
git add crates/tui/Cargo.toml crates/tui/src/lib.rs Cargo.lock
git commit -m "feat(tui): live swarm — inline pane during fan-out + Ctrl-V/\`/viz\` full view" -- crates/tui/Cargo.toml crates/tui/src/lib.rs Cargo.lock
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Final verification (after Task 6 lands)

- [ ] `cargo test -p entheai-viz -p entheai-config -p entheai-tui` → all green.
- [ ] **Manual (spec §9):** `entheai --fanout "add a CONTRIBUTING.md and a .editorconfig"` shows an inline swarm whose nodes go `◻→◐→✓/✗` live; `Ctrl-V` opens the full swarm and returns to chat; the inline pane is gone (0 rows) once the run finishes; idle CPU stays low (reuses the existing idle-frugal redraw).
- [ ] Confirm `crates/viz` has **no** dependency on `entheai-orchestrator` or `tokio` (`cargo tree -p entheai-viz` shows only `ratatui`): the model stayed pure.

## Notes for the executor

- **`crates/viz` is orchestrator-free by design.** The TUI does the `FanoutEvent`→mutator mapping. Never add an `entheai-orchestrator` dep to `crates/viz`.
- **Slice 2 (shader) is out of scope** — no `wgpu`, no Kitty-graphics emit here. `term::graphics_capable()` ships now only so the TUI can label/gate future viz.
- **Tasks 1–5 are safe on a red `main`** (new crate + config). **Task 6 waits for a green `entheai-tui`.**
- ratatui 0.29 API drift: if `Canvas`/`ctx.print`/`Buffer::content()` differ from the snippets, adapt to the installed 0.29 API — the *behavior* (nodes/edges/glyphs, collapse-when-idle, Ctrl-V toggle) is the contract, and the tests encode it.
