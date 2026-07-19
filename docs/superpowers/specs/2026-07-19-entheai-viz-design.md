# entheai viz — "the swarm" + shader identity — Design

**Date:** 2026-07-19 · **Status:** approved design, pre-plan
**Scope:** the **viz** pillar of v0.2 — visual identity as a first-class feature. Delivered in two slices; **this pillar's first implementation plan targets Slice 1 (the swarm)**. The **codebase graph** is explicitly a *different* pillar (needs codebase-memory-mcp) and out of scope here.

**Inspiration:** **Crush** (animation, feel) + **jcode** (ultra performance — idle-frugal, no per-frame alloc). The viz makes entheai *look* like what it is: an orchestrator that fans out into a swarm.

**Decisions locked in brainstorming (2026-07-19):**
- Render **both** the live agent-activity **swarm** (functional) and an ambient **shader** background (identity).
- **Hybrid, Ghostty-targeted:** the swarm draws in pure ratatui (works in any terminal); the shader is native **wgpu → Kitty graphics** (Ghostty/Kitty/WezTerm), with a cell-gradient fallback.
- **Phased:** ship the swarm first; layer the shader in second.
- **Layout:** an inline mini-pane during fan-out **and** a first-class full-screen viz-mode (`Ctrl-V` / `/viz`).

**Technical facts confirmed (Valyu web search, 2026-07-19):**
- **Ghostty** officially supports the **Kitty graphics protocol** (ghostty.org/docs/features) — the shader layer's transport.
- **ratatui `Canvas`** provides `Marker::Braille` (highest resolution, 2×4 dots/cell, single fg color), `Marker::HalfBlock` (fg+bg per cell — good for gradient fills), `Dot`, `Block`; `.paint(|ctx| …)` draws points/lines/shapes. → the swarm needs no graphics protocol.
- **Prior art** for image→Kitty from Rust: `viu`/`viuer`, `chafa`. → the shader path is "render an RGBA frame → transmit as a Kitty graphics image behind text."

## 1. Purpose

Turn fan-out from invisible-plumbing into the product's signature moment: you *watch* the orchestrator decompose a task and fan out to model-matched sub-agents in isolated worktrees, live — then optionally sit that swarm on an ambient animated shader. All inside the existing single-window flow, idle-frugal.

## 2. Scope

**In:**
- **Slice 1 — the swarm** (the plannable, shippable unit): a `viz::SwarmModel` folded from the existing `FanoutEvent` stream, rendered as a ratatui `Canvas`; a TUI inline pane during fan-out + a full-screen viz-mode toggle; `[viz] swarm` config. Pure ratatui — works in **every** terminal.
- **Slice 2 — the shader** (follow-up plan): a wgpu offscreen renderer → Kitty-graphics image composited **behind** text; built-in shaders; terminal-capability gating; cell-gradient fallback; `[viz] shader*` config.

**Out (explicit):** the **codebase knowledge graph** (its own pillar; depends on codebase-memory-mcp / `:9749`); embedding a browser/webview; pixel-perfect GPU UI beyond the shader background; per-shader authoring UX.

## 3. Architecture — new `crates/viz`

A crate **decoupled from the TUI** (matches the master spec's crate list) so the model + layout are testable without a terminal.

| Unit | What it does | Depends on |
|---|---|---|
| `viz::SwarmModel` | **Pure state machine** with semantic mutators — `decompose(tasks)`, `coder_started(index, role, task)`, `coder_finished(index, committed, status)`, `integrating()`, `done()` — folding into a graph: an orchestrator node + one node per sub-task `{ index, role, task, status, committed, tokens? }`; statuses `Pending / Running / Done / Failed`; aggregate counts + elapsed time (provided by the caller — the model calls no clock, so it stays deterministic for tests). **No rendering, no I/O, and no dependency on the orchestrator crate** — the TUI maps each `FanoutEvent` to the matching mutator, keeping `viz` a standalone, cheaply-testable crate. | std only |
| `viz::swarm::render(model, area, buf, marker, frame)` | Draws the model onto a ratatui `Canvas`: nodes as Braille points, edges orchestrator→node as lines, status glyphs `◻ ◐ ✓ ✗`, labels truncated to width, a pulse on `Running` nodes derived from `frame`. **Same fn for inline (small `area`) and full (large `area`).** | `ratatui` |
| `viz::term` | `graphics_capable() -> bool` — detects Kitty-graphics support from `$TERM_PROGRAM` (`ghostty`, `WezTerm`) and `$TERM` (`xterm-kitty`). | std env |
| `viz::shader` *(Slice 2)* | wgpu offscreen render → RGBA frame → Kitty-graphics emit behind text; **first built-in `Raindrop`** — an ambient rain-on-glass effect (animated droplets + refraction/trails, inspired by [SardineFish/raindrop-fx](https://github.com/SardineFish/raindrop-fx)) — plus `Random`; the Kitty escape encoder; cell-gradient fallback surface. | `wgpu`, `viz::term` |

`FanoutEvent` variants the TUI maps to `SwarmModel` mutators (all already emitted): `Decomposed { tasks: Vec<(role, task)> }` → `decompose` (seed `Pending`); `CoderStarted { index, role, task }` → `coder_started` (`Running`); `CoderFinished { index, committed, status }` → `coder_finished` (`Done`/`Failed`); `Integrating` → `integrating`; `Done` → `done`. No new events required; `viz` never imports `FanoutEvent`.

## 4. Data flow

```
fan-out ─FanoutEvent─▶ TUI maps evt→mutator ─▶ SwarmModel ─▶ TUI renders:
  Decomposed   → decompose()      Pending      · inline pane while fan-out active
  CoderStarted → coder_started()  Running ◐    · Ctrl-V / /viz → full viz-mode
  CoderFinished→ coder_finished()  Done ✓/Failed ✗
  Integrating/Done → integrating()/done()
                                   (Slice 2: shader renders behind all of it/frame)
```

The TUI already forwards `FanoutEvent`s to the plan pane; the swarm feeds the **same** stream (translated to mutator calls) into a `SwarmModel`. One source of truth, two views (plan list + swarm graph).

## 5. TUI integration (`crates/tui`)

- New state: `swarm: viz::SwarmModel`, `view: ViewMode { Chat, Swarm }`. Fed every `FanoutEvent`.
- **Inline pane:** a bordered `swarm` region between scrollback and the input, shown **only while a fan-out is active**, height `min(nodes+2, cap)`, collapses to **0 rows** when idle so the input never jumps. (Same discipline as the plan pane from the TUI-transparency pillar.)
- **Full viz-mode:** `Ctrl-V` (and the `/viz` command) toggles `ViewMode::Swarm` — the main content area becomes the large swarm Canvas with per-agent detail (role · status · tokens · branch · merge). Any key/`Ctrl-V`/`/viz` returns to `Chat`. Input/streaming keep running underneath.
- **Performance (jcode):** reuse the existing animation tick + dirty-flag redraw from the TUI-transparency pillar — redraw only on a real change or an active pulse; cache the Canvas geometry, rebuild on model-change or resize; no per-frame allocation in the hot path.

## 6. Config `[viz]`

```toml
[viz]
swarm = true               # slice 1: the fan-out swarm (inline + Ctrl-V full view)
# slice 2:
shader = "off"             # off | raindrop | random | <custom>   (first shader: raindrop = rain-on-glass, raindrop-fx-inspired)
shader_enabled = false     # opt-in; auto-disabled on a non-Kitty terminal regardless
```

Defaults keep the swarm on and the shader off (opt-in) so slice 1 changes nothing about default startup cost.

## 7. Error handling / fallback

- **No fan-out active:** inline pane collapses; full viz-mode shows an idle hint ("no swarm running — start one with `--fanout` or `/fanout`").
- **Non-Kitty terminal:** `viz::term::graphics_capable()` is `false` → shader forced off; optional animated cell-gradient (HalfBlock) fallback, or nothing. The **swarm is unaffected** (pure ratatui).
- **wgpu init failure (Slice 2):** shader off, `log::warn!`, TUI continues. The viz layer must never block or crash the agent loop or the TUI.

## 8. Testing

- **`SwarmModel`** (pure, deterministic): a `FanoutEvent` sequence → expected nodes/statuses/counts (`Decomposed` seeds N `Pending`; `CoderStarted` → `Running`; `CoderFinished{status}` → `Done`/`Failed`; empty stream → empty model; out-of-order/duplicate indices handled).
- **`swarm::render`** via ratatui `TestBackend`: empty model → no nodes; N nodes placed within `area`; labels truncated to width; correct glyph per status; small vs large `area` both render without panic.
- **`viz::term`**: `$TERM_PROGRAM=ghostty` / `WezTerm`, `$TERM=xterm-kitty` → capable; plain `xterm-256color` → not.
- **TUI**: `Ctrl-V` toggles `ViewMode`; the inline pane is 0 rows when no fan-out is active and non-zero during one (assert via the recording harness).
- **Slice 2**: offscreen render yields a correctly-sized, non-empty RGBA frame; the Kitty-graphics encoder emits valid escape bytes for a known small image (unit-test the encoder); fallback path taken when `!graphics_capable()`.
- **Manual:** `--fanout` shows the inline swarm animating live; `Ctrl-V` expands to the full view and back; (Slice 2) shader toggles on Ghostty and degrades cleanly elsewhere; idle CPU stays low.

## 9. Success criteria

- Running `entheai --fanout "<task>"` shows a **live swarm**: an orchestrator node fanning out to model-matched sub-task nodes that transition `◻→◐→✓/✗` in real time, off the existing `FanoutEvent` stream — inline during the run.
- `Ctrl-V` / `/viz` opens a **full-screen swarm** with per-agent detail and returns to chat without restarting; the inline pane collapses to zero rows when idle.
- The swarm works in **any** terminal (pure ratatui) and the render loop stays **idle-frugal** (no busy-spin).
- **Slice 2:** on Ghostty, the **`Raindrop`** shader (rain-on-glass with refraction/trails, inspired by SardineFish/raindrop-fx) animates **behind** the text with negligible idle cost; on a non-Kitty terminal it auto-disables with a clean fallback — proving the wgpu→Kitty path with one shader, exactly as the master spec's risk note prescribes.

## 10. Non-goals (v1)

The codebase graph (separate pillar) · a full shader library (one shader proves the path) · custom-shader authoring UX · embedding the codebase-memory 3D graph UI · GPU-composited foreground UI (only the background shader is pixels; all UI stays ratatui).
