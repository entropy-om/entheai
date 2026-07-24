---
id: gallery
title: "Visualization gallery"
navTitle: "Gallery"
group: "The visual TUI"
order: 3
badgeText: "Visualizations"
badgeColor: cyan
---

`entheai` is visual by design — built on `ratatui` with braille 3D projections, custom color-harmony themes, and dynamic telemetry streams over NATS.

## 1. The Zen View (`/zen` or `Ctrl-G`)

The operator's canvas: the entire terminal area transforms into a full-canvas living field.

- **Singularity Core**: A breathing center light whose brightness and pulse rate scale with cognitive activity (`BrainState::vitality()`).
- **Orbiting Faculties**: Model, tools, and context rendered as luminous bodies tethered to the center.
- **Frozen Constellation**: Counter-rotating ring representing active `frozen/*.md` doctrine. Active nodes flare and label.
- **Current-Awareness Motes**: Drifting particle field colored by source origin — **dogfood gold** (genetic corpus), **Valyu cyan** (academic/web search), **WorldMonitor green** (living world news), and **violet** (internal).
- **Whisper Reply & Input**: The assistant reply materializes directly in the field, fanning out into motes as it dissolves into soil.

*Capture Note: Recorded live in terminal space using Ghostty (v0.2.0+) with `entheai /zen` active on Apple Silicon (M1 Max, 120 FPS render target).*

## 2. Brain-Ring Constellation

The always-on compact side panel (`/brain`, `[viz] brain = true`).

- **FACULTIES**: Braille 3D pseudo-rotation tracking live activity in `model`, `tools`, and `context`.
- **FLEET**: Live presence ring displaying connected remote federation nodes over NATS (`entheai-bus`).
- **IDLE SENSOR**: Bypasses MCP using direct system idle polling — linear rotation slowdown as the user steps away from the keyboard, returning instantly to full speed on input.
- **FOOTER**: Live `wk N` (active fan-out workers), `nats ●/○` (bus status), `ctx %` (context window saturation), and `cmp` (Marqant compression ratio).

*Capture Note: Captured during an interactive TUI session with side panel width set to `brain_width = 26`.*

## 3. The 4 Ambient Palettes (`/theme`)

All themes use global, machine-validated source identity colors (worst deutan $\Delta E \ge 9.1$, normal-vision $\Delta E \ge 18.0$) while restyling ambient surfaces:

| Theme | Aesthetic | Visual Tone |
|---|---|---|
| `entheia` | Signature teal default | Clean, crisp, high-contrast cyan/teal glow |
| `ember` | Night fire | Warm amber, deep gold, obsidian darkness |
| `verdant` | The garden | Deep emerald, moss, organic growth hues |
| `void` | Monochrome austerity | Stark black & white with lineage gold thread |

*Capture Note: Generated via `/theme <name>` in the interactive TUI and validated using `entheai-viz::theme` luminance tests.*

## 4. Fan-Out Orbits & Swarm Graph

Visualized during parallel execution (`entheai --fanout "<task>"` or `Ctrl-V`).

- **Orchestrator Hub**: DeepSeek V4 Pro orchestrator at the hub planning and routing.
- **Model-Matched Sub-Agents**: Orbits of parallel coders running in isolated git worktrees.
- **Status Indicators**: Pending (dim), running (cyan pulse), done/verified (gold flash), failed (magenta alert).
- **Empirical Gate & Merge Seals**: Completed branches pass `./scripts/check.sh` and receive a deterministic SHA-256 `MergeSeal` before merging.

*Capture Note: Captured live during a 4-worker parallel refactor batch on entheai.*

## 5. The Kin Ring (`[kin]`)

The Zen field's outermost, slowest-turning ring (`[kin] nodes = ["https://riva.vaked.dev/"]`).

- **Flowing Kin**: Reachable sibling nodes breathe in the theme's kin color and display their host label (e.g. `riva`).
- **Unreachable Kin**: Nodes failing liveness check sit as dim, dark points — honest status, never faked.
- **Zero-Block Background Task**: Background worker polls status endpoints once per `poll_secs` (default 120s) without blocking UI execution.

*Capture Note: Captured with live NATS & HTTP status polling enabled across tailnet nodes.*
