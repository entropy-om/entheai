# entheai viz Slice 2 — Action Plan (from deep-research synthesis)

> Multi-agent synthesis of `docs/research/deepresearch.md` (viz layer, 162 sources, $15 run). Generated 2026-07-19. Feeds the Slice 2 spec.

---

# entheai Slice 2 — Ambient Raindrop Shader: Actionable Plan

## 1. Slice 2 shader-approach DECISION

**Decision: Path A — Ghostty native `custom-shader` (GLSL ES 3.0 rain-on-glass). Ship Path C (ANSI Perlin + HalfBlock) as the universal fallback. Do NOT build Path B (wgpu → Kitty) for Slice 2.**

### Why Path A wins for entheai

- **Idle-frugal by construction.** Ghostty runs the shader GPU-resident at a fixed ~1% CPU / ~2% GPU *regardless of visual complexity*, with zero PTY bandwidth and zero per-frame heap allocation on entheai's side (§1, §8, §9). This is the single strongest match to the jcode-grade no-per-frame-alloc priority — entheai literally does no per-frame work; it just draws normal TUI text and Ghostty distorts it.
- **Behind-text is native, not a compositing hack.** The shader draws beneath glyphs by design; `iChannel0` = the rendered terminal texture, so the rain self-distorts the actual text underneath. Legibility becomes a *config* concern (refraction 0.08, blend 0.3, `minimum-contrast=1.3`), not a z-index gamble (§4, §8, §9).
- **Single-window flow preserved.** No second render pipeline, no synchronized-update image transmit dance, no ratatui-backend swap. The existing ratatui+crossterm stack is untouched (§8).
- **Works on all Ghostty versions.** The custom GLSL background shader is supported v1.0–v1.3+; it does NOT need the v1.2 behind-text z-index fix (#7671) that the Kitty-graphics route depends on (§9).
- **The report ships a complete, legibility-tuned `rain_on_glass.glsl`** ready to drop in (§4 reusable snippet).

### Why NOT Path B (wgpu → Kitty)

- **Unresolved Apple Metal driver leak.** wgpu #8768 leaks ~1 MB / 10s from render-pass creation on Metal — ~8.6 GB/day at 60 FPS. It is a *driver* bug, not a wgpu bug, so pinning a newer wgpu does not fix it. Fatal for a long-running agent (§3, §11). This alone disqualifies Path B as the common-case path.
- **Ghostty has no Kitty animation frames** (#5255, no ETA) → mandatory per-frame `a=T` full re-transmission, the bandwidth-heavy path (7.3–89.6 MB/s depending on encoding), plus the Ghostty image-ID replacement bug (#6711) forcing delete-then-upload every frame (§1, §6, §9).
- **The core behind-text assumption is UNVERIFIED.** No end-user test of a full-screen shader at `z=-1,073,741,825` (BELOW_BG) exists; #7671 was fixed in v1.2 but never empirically confirmed for this exact path (§11).
- **Materially heavier:** 24 FPS, ~5% CPU / 3–8% GPU, 50–100 MB vs Ghostty's GPU-managed 60 FPS (§8).

### The lock-in trade-off (accepted)

Path A is **Ghostty-only** — no portability (§1 risk). We accept this because entheai *targets* Ghostty on Apple Silicon. The graceful-fallback priority is satisfied by **Path C** (ANSI truecolor Perlin noise + HalfBlock: 30 FPS, 2–6% CPU, 0 GPU, ~40–50 MB, works everywhere — §7), NOT by Path B. Path B is deferred indefinitely; only reconsider if wgpu #8768 is confirmed fixed AND a real user base emerges on Kitty/WezTerm.

### One caveat to spike first

Path A also distorts the **swarm-graph glyphs** (Slice 1), since the shader warps the whole terminal texture. Refraction 0.08 is the only stated safeguard — **verify graph legibility empirically on-device** (§8 risk). This is a spike, not a blocker.

---

## 2. Concrete crates + Cargo.toml (Path A + Path C)

Path A needs **no new runtime crates** — the shader is a Ghostty config file. entheai just ships the `.glsl` and (optionally) enables it. Path C needs only what the TUI already has plus `noise`/`rand`.

```toml
[dependencies]
# --- existing TUI/graph stack (Slice 1, keep) ---
ratatui        = "0.30"              # Canvas, Marker::HalfBlock, apply_buffer (§5, §8)
crossterm      = "0.28"              # backend; BeginSynchronizedUpdate/DECSET 2026 built in (§6, §8)
petgraph       = "0.8"              # StableGraph<AgentState,()> for swarm (§5)
ascii-petgraph = "0.2"              # force-directed layout → ratatui widget (§5)
tachyonfx      = "0.25"             # cell-level node-state effects (§2, §5)

# --- capability detection + cell-size discovery (Path A gating + Path C) ---
libc           = "0.2"              # TIOCGWINSZ ioctl for cell pixel size (§6)
signal-hook     = "0.3"              # emit Kitty delete-all on exit/panic IF any image path is used (§7)

# --- Path C universal fallback (ANSI Perlin + HalfBlock) ---
# reference impls: perlin-terminal, halo (frame-diffing), termflix (§7)
# noise generated in-Rust; pick one:
# noise / rand for the Perlin field — pre-allocate all state before the loop (§7)
```

**Explicitly NOT in the Slice-2 dependency set** (Path B only — do not add unless Path B is resurrected): `wgpu`, `bytemuck`, `image`, `png`, `kitty-graphics-protocol`, `kittage`, `ratatui-wgpu`, `ratatui-image` (§2, §8). `ratatui-image`'s `Picker` probe is the *one* useful piece if you want a ready-made protocol/font-size detector, but for Path A/C the `libc` TIOCGWINSZ + env-var route is sufficient and lighter (§2, §6).

**Ghostty config to ship** (§4, §8 verbatim):
```
font-family = "JetBrainsMono Nerd Font"
font-thicken = true
background = #0f0e1d          # dark base = legibility safety net (§1)
foreground = #e1cba6
custom-shader = ~/.config/entheai/shaders/rain_on_glass.glsl
custom-shader-animation = always
minimum-contrast = 1.3        # keep ≤1.3 (>1.6 = Linux bug #8745) (§4, §7)
background-opacity = 1.0       # keep 1.0; encode opacity in shader (conflict #4835) (§7, §8)
```

**Shader source**: use the legibility-tuned `rain_on_glass.glsl` verbatim from §4 (ldSBWW 3-iteration port; refraction 0.08, 30% blend, luminance dark-mask). Do **not** port SardineFish (per-drop tracking = per-frame state, overkill) (§4).

---

## 3. Slice 1 swarm refinements worth doing

Slice 1 is shipped; these are refinements the report surfaces (§5, §8):

1. **Confirm/keep `Marker::HalfBlock`**, not Braille. Braille has a documented 24% font gap (Warp #9696) and an integer-overflow panic in `BrailleGrid::new()` at ~1000×1500 cells (#1449). HalfBlock's dual fg+bg per cell lets you encode orchestrator color on one half, agent-status on the other (§5).
2. **Event-driven redraw split** — separate `AgentJoined`/`AgentLeft` (→ `add_node`/`add_edge` + `run_simulation()`) from `AgentStatusChanged` (→ `set_node_border_color` only, `app.dirty=true`, **no relayout**). Only `terminal.draw()` when `app.dirty`. This is the idle-frugal contract (§5, §8).
3. **crossterm poll 50 ms (20 FPS) while animating; block indefinitely when `is_stable()`** for zero idle CPU (§5).
4. **Tune ascii-petgraph for sparse fan-out** — raise `repulsion_constant` to ~15000–20000 and `gravity` above 0.3 so 5–50 agents don't drift off-screen (defaults 10000/0.3 are tuned for denser graphs; `damping` 0.85 is not builder-exposed) (§5).
5. **Manual label collision detection** — `ctx.print()` draws unconditionally on top with no de-overlap and ignores `ctx.layer()`. Track node bounding boxes before emitting labels (§5).
6. **tachyonfx for join/leave transitions** — `fade_to`, `hsl_shift`, `dissolve`/`coalesce`/`glitch` on node state changes. It operates on already-rendered cells, complements the graph (§2, §5).
7. **Skip viewport culling** at 5–50 agents (far below the N≥3000 threshold), but keep the #1449 panic bound in mind (§5).
8. **Benchmark `run_simulation()` at N>50** — no published convergence timings exist; measure before trusting at scale (§11).

---

## 4. Compositing + fallback strategy

**On Path A, entheai does almost no compositing** — Ghostty owns it. The compositing machinery below matters mostly for Path C and for the (deferred) Path B, but the detection/teardown discipline still applies.

### Detection ladder (startup `TerminalCapabilities::detect()`) — §7, §8

```
TERM_PROGRAM == "ghostty"        → Path A (GhosttyNative custom-shader)   [skip Kitty probe]
else Kitty a=q APC probe (100ms) → Path B (deferred; treat as Path C for now)
else                             → Path C (ANSI Perlin + HalfBlock)
force Path C when: $STY (GNU Screen), SSH session (default), no graphics reply
also read $TMUX, $ZELLIJ
```
- Probes need real ~100 ms timeouts or startup hangs on silent terminals (§8). Silence = no support → fall back (§1).
- Consider a log-odds/Bayesian evidence ledger over boolean flags (§1) — nice-to-have, not required for v1.

### Synchronized output (DECSET 2026) — §6
- Only relevant if entheai ever transmits images itself (Path B/C image compositing). For **Path A it's unnecessary** — Ghostty composites the shader natively every vsync.
- If used: `BeginSynchronizedUpdate` → (delete prev image) → transmit frame at BELOW_BG → `terminal.draw()` → `EndSynchronizedUpdate`. Probe `CSI?2026$p`, accept `;1$y`/`;2$y`; GNOME (mode 4) and GNU Screen = unsupported (§6).

### Cell-size discovery — §6
Query at **runtime**, never trust config. Ghostty on Retina reports **physical** px via TIOCGWINSZ (2×) — render any image in physical px or it letterboxes. Order: `CSI 16 t` → TIOCGWINSZ ioctl on `/dev/tty` → hardcode (8,16). Path A doesn't need this (Ghostty sizes the shader), but Path C HalfBlock rendering benefits.

### Image persistence / teardown — §7
- If any image path is active: install a `panic_hook` + `signal-hook` handler emitting the Kitty delete-all `\x1b_Ga=d,d=A;\x1b\\` on exit/crash to prevent unbounded memory growth (Alacritty/Warp don't clear; #5683).
- **Path A needs none of this** — no images transmitted.

### tmux / SSH gotchas — §7
- tmux: needs `allow-passthrough on` (3.2+); multi-chunk images break at absolute coords → use Unicode placeholder mode (U=1). Ghostty **crashes** rendering Kitty images in tmux with mouse enabled, esp. >100 KB (#11909). **Path A sidesteps all of this** — the shader is Ghostty-config-level, not a per-session escape stream. But note: `custom-shader` under tmux still renders (it's Ghostty painting its own window), so Path A degrades gracefully under multiplexers where Path B would corrupt.
- SSH: treat as non-graphics (Path C) by default; Kitty graphics only reliable via `kitty +kitten ssh`.

### Path C implementation notes — §7
Pre-allocate all Perlin state arrays before the event loop (no per-frame alloc); emit only changed cells (frame-diffing ≈ 70% fewer escape bytes, per halo); cap 30 FPS; `Color::Rgb` half-blocks.

---

## 5. Recommended BUILD SEQUENCE

**Slice 2a — SPIKE: prove the primary path (½–1 day, gating).** §11
- Load `rain_on_glass.glsl` in Ghostty via `custom-shader` + `custom-shader-animation=always` on-device (Apple Silicon, Ghostty ≥1.2).
- Confirm: (a) rain renders behind text; (b) **swarm-graph glyphs stay legible** under refraction 0.08 (§8 risk); (c) `minimum-contrast=1.3` holds contrast. Tune the two knobs (refraction, blend) empirically if needed.
- **Exit criterion:** legibility acceptable → commit to Path A. If not, adjust shader constants (never touch Path B).

**Slice 2b — Capability detection + config plumbing.** §7, §8
- Implement `TerminalCapabilities::detect()` (env short-circuit + probes + tmux/SSH forcing).
- Decide + implement **shader ownership** (see Open Decisions): ship a config snippet vs. write/merge into user Ghostty config vs. document. entheai cannot inject `custom-shader` per-session at runtime (§9 risk).

**Slice 2c — Path C universal fallback.** §7
- Rust Perlin + HalfBlock background, pre-allocated state, frame-diffed, 30 FPS cap, event-gated. This is the "graceful fallback on non-graphics terminals" deliverable.

**Slice 2d — Teardown safety + polish.** §7
- panic/signal cleanup hooks (only strictly needed if any image path lands, but cheap insurance), idle-gating review, memory-growth smoke test.

**Slice 1 refinements — parallel/independent** (§3 above): fold in whenever; they don't block Slice 2. Prioritize (2) event-driven redraw split and (4) physics tuning.

**Deferred indefinitely — Path B (wgpu→Kitty).** Only spec if wgpu #8768 is confirmed fixed post-v30 AND non-Ghostty graphics users materialize (§3, §11).

---

## 6. OPEN DECISIONS for the user

1. **Who owns enabling the shader?** entheai cannot inject `custom-shader` at runtime — it's a Ghostty *config-file* setting (§9 risk). Options: (a) ship a copy-paste config snippet + doc; (b) an `entheai doctor`/setup command that writes/merges the shader path + tuning into `~/.config/ghostty/config`; (c) bundle the `.glsl` and just document. **Recommend (b)** for single-window "it just works" UX — needs your call on how aggressively entheai edits user config.

2. **Fallback ambition on non-Ghostty terminals.** Confirmed: Path C (ANSI Perlin), NOT Path B. Do you want the ambient effect *at all* off-Ghostty, or is "plain TUI, no shader" acceptable? Path C is ~1–2 days; skipping it means non-Ghostty users get no ambient layer (still graceful, just absent) (§7, §1).

3. **Minimum Ghostty version to support.** Path A works v1.0+, but v1.1.4 has PNG artifacts and behind-text z-index needs v1.2 (irrelevant to Path A, relevant only if Path B ever returns). **Recommend requiring Ghostty ≥1.2.0** and gating with a version check + friendly message (§7, §9).

4. **Does the shader distorting the swarm graph read as feature or bug?** The rain warps graph glyphs too (§8 risk). If undesirable, we'd need to spec conditional shader disable while the graph is focused — which Ghostty's config-level shader *cannot* do at runtime (§9). Decide in Slice 2a spike: accept the ambient warp, or drop the shader on graph-heavy screens (which Path A can't toggle live — a point in Path C's favor if live toggle is a hard requirement).

5. **Idle-frugal vs `custom-shader-animation = always`.** `always` animates even unfocused/idle, which technically fights the idle-frugal goal (§4 risk). Alternative: default `= true` (animate only when focused). **Recommend `= true`** unless you specifically want ambient motion on an unfocused window — cheaper and honors idle-frugal. Your call on the aesthetic.