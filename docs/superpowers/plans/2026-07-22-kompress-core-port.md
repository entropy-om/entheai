# Kompress-Core Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: this repo's own `entheai --fanout` (agy/AgyExecutor) drives execution, task-by-task, in place of superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port kompress-ultra's `kompress-core` compression pipeline into entheai as a native, in-process `Marqant` backend for prompt-processing, and add a compression "pulse" readout to the existing brain panel — sourced from entheai's own live state, not kompress-ultra's personal mygraph/lore data.

**Architecture:** Vendor `kompress-core` (composer → pruner → rewriter → circulator, λ=3.0 asymmetric-loss scoring) as a new workspace crate with zero unused deps. Wrap its `Pipeline` behind entheai's existing `Marqant` trait (`crates/memory-pp/src/marqant.rs`) as a third backend selectable via a new `marqant_backend` config field, mirroring the existing `mesh_backend` pattern. Then extend `entheai_viz::BrainState` (already tracks `ctx_pct`, `frame`, `frozen`) with a compression-pulse field driven by each `PipelineResult`, rendered as an extra line in the existing brain-panel footer in `crates/viz/src/brain.rs` — no changes to the `crates/tui/src/lib.rs` render logic itself beyond one state-set call, since that file is a flagged hotspot (95th %ile churn).

**Tech Stack:** Rust, existing entheai workspace conventions (serde, anyhow, async-trait, ratatui). No new external dependencies — kompress-core needs only `serde`, `serde_json`, `anyhow` (all already workspace deps); `chrono`, `sha2`, `thiserror` from its original Cargo.toml are unused in the code and dropped.

## Global Constraints

- Do not port `kompress-brain`'s `persons.rs` (Ralph/Lodri/Krengel/Cosmos lore), `loader.rs` (hardcoded personal `~/Desktop/ideas/...` path), or `mygraph.rs` — none of it is portable/generic; only the *pattern* (a compact liveness line built from live stats) is adapted.
- `kompress-core` source lives at `/Users/peter.lodri/workspace/peterlodri-sec/kompress-ultra/crates/kompress-core/src/` — copy verbatim, do not hand-retype (risk of transcription bugs); the plan gives exact `cp` commands.
- Every new/modified file must pass `cargo test -p <crate>` and `cargo clippy -p <crate> -- -D warnings` before its commit step.
- Follow existing naming: the trait is `Marqant`, the new impl is `KompressMarqant` (functional name — no lore names in code identifiers).

---

### Task 1: Vendor the kompress-core crate

**Files:**
- Create: `crates/kompress-core/Cargo.toml`
- Create: `crates/kompress-core/src/lib.rs`
- Create: `crates/kompress-core/src/types.rs`
- Create: `crates/kompress-core/src/loss.rs`
- Create: `crates/kompress-core/src/pruner.rs`
- Create: `crates/kompress-core/src/rewriter.rs`
- Create: `crates/kompress-core/src/circulator.rs`
- Create: `crates/kompress-core/src/composer.rs`
- Create: `crates/kompress-core/src/pipeline.rs`
- Modify: `Cargo.toml:2` (workspace `members` list)

**Interfaces:**
- Produces: `kompress_core::{Pipeline, PipelineResult, ContextUnit, LAMBDA, TARGET_RATIO}` — consumed by Task 2.
  - `Pipeline::new() -> Pipeline`
  - `Pipeline::run(&self, inputs: Vec<String>) -> anyhow::Result<PipelineResult>`
  - `PipelineResult { units: Vec<ContextUnit>, input_tokens: usize, output_tokens: usize, compression_ratio: f64, target_ratio: f64 }`
  - `ContextUnit { id: String, content: String, score: f64, layer: [i8; 3], token_count: usize, is_critical_syntactic: bool }`

- [ ] **Step 1: Copy the crate source verbatim**

```bash
mkdir -p crates/kompress-core/src
cp ../kompress-ultra/crates/kompress-core/src/lib.rs crates/kompress-core/src/lib.rs
cp ../kompress-ultra/crates/kompress-core/src/types.rs crates/kompress-core/src/types.rs
cp ../kompress-ultra/crates/kompress-core/src/loss.rs crates/kompress-core/src/loss.rs
cp ../kompress-ultra/crates/kompress-core/src/pruner.rs crates/kompress-core/src/pruner.rs
cp ../kompress-ultra/crates/kompress-core/src/rewriter.rs crates/kompress-core/src/rewriter.rs
cp ../kompress-ultra/crates/kompress-core/src/circulator.rs crates/kompress-core/src/circulator.rs
cp ../kompress-ultra/crates/kompress-core/src/composer.rs crates/kompress-core/src/composer.rs
cp ../kompress-ultra/crates/kompress-core/src/pipeline.rs crates/kompress-core/src/pipeline.rs
```

Note: `../kompress-ultra/crates/kompress-core/src/tests/mod.rs` referenced by `#[cfg(test)] mod tests;` in the original `lib.rs` does not exist as a file in the source tree (each module has its own inline `#[cfg(test)] mod tests { ... }` block instead) — the `mod tests;` line and its directory do not need to be copied. Confirm with:

```bash
ls ../kompress-ultra/crates/kompress-core/src/tests/ 2>&1
```

If that directory is empty or missing, remove the `mod tests;` re-export concern — the per-file `#[cfg(test)]` blocks already copied are the real tests.

- [ ] **Step 2: Write the trimmed Cargo.toml**

```toml
[package]
name = "kompress-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
```

Write this to `crates/kompress-core/Cargo.toml`.

- [ ] **Step 3: Add the crate to the workspace**

In `Cargo.toml` at the repo root, find the `members = [...]` list (currently starts `members = ["crates/config", "crates/providers", "crates/core", ...`) and add `"crates/kompress-core"` to it (alphabetical position doesn't matter — match the existing list's ordering convention, which is roughly introduction order, so append near the end before the `bin/*` entries).

- [ ] **Step 4: Build and test**

```bash
cargo test -p kompress-core
```

Expected: all tests from `loss.rs` (8 tests), `pipeline.rs` (2 tests) pass — no compile errors. `chrono`/`sha2`/`thiserror` are gone from the Cargo.toml; the crate must still compile clean since the original code never referenced them.

- [ ] **Step 5: Clippy**

```bash
cargo clippy -p kompress-core -- -D warnings
```

Expected: clean. Fix any warnings inline (e.g. needless clones) without changing behavior.

- [ ] **Step 6: Commit**

```bash
git add crates/kompress-core Cargo.toml Cargo.lock
git commit -m "feat(kompress-core): vendor compression pipeline from kompress-ultra"
```

---

### Task 2: KompressMarqant — native Marqant backend

**Files:**
- Modify: `crates/memory-pp/Cargo.toml` (add `kompress-core` dependency)
- Modify: `crates/memory-pp/src/marqant.rs`
- Modify: `crates/memory-pp/src/lib.rs:18` (export `KompressMarqant`)

**Interfaces:**
- Consumes: `kompress_core::Pipeline`, `kompress_core::PipelineResult` from Task 1.
- Consumes: `Marqant` trait, `PpError` from existing `crates/memory-pp/src/marqant.rs` / `crates/memory-pp/src/error.rs`.
- Produces: `pub struct KompressMarqant` implementing `Marqant`, exported as `entheai_memory_pp::KompressMarqant` — consumed by Task 3.

- [ ] **Step 1: Write the failing test**

Add to `crates/memory-pp/src/marqant.rs`, inside the existing `#[cfg(test)] mod tests` block (create one if none exists yet — check the bottom of the file first; if a `mod tests` block already exists, add these as new `#[tokio::test]` functions inside it):

```rust
    #[tokio::test]
    async fn kompress_marqant_reduces_filler_and_keeps_critical_tokens() {
        let m = KompressMarqant::new();
        let findings = "This is basically just a very simple finding about /usr/bin/cargo \
                         and it is obviously really quite verbose for no reason at all.";
        let brief = m.compress(findings, Duration::from_millis(500)).await.unwrap();
        assert!(brief.contains("/usr/bin/cargo"), "critical file path must survive: {brief:?}");
        assert!(brief.len() < findings.len(), "output should be shorter than input: {brief:?}");
    }

    #[tokio::test]
    async fn kompress_marqant_empty_input_yields_empty_brief() {
        let m = KompressMarqant::new();
        let brief = m.compress("", Duration::from_millis(500)).await.unwrap();
        assert!(brief.trim().is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-memory-pp kompress_marqant -- --nocapture`
Expected: FAIL with `cannot find type KompressMarqant in this scope` (or similar — the type doesn't exist yet).

- [ ] **Step 3: Add the dependency**

In `crates/memory-pp/Cargo.toml`, in the `[dependencies]` section, add:

```toml
kompress-core = { path = "../kompress-core" }
```

- [ ] **Step 4: Implement KompressMarqant**

Add to `crates/memory-pp/src/marqant.rs`, after the existing `SubprocessMarqant` impl block:

```rust
/// Native, in-process compressor backed by `kompress-core`'s pipeline
/// (composer → pruner → rewriter → circulator, λ=3.0 asymmetric-loss scoring).
/// No subprocess, no external `mq` binary — an alternative to `SubprocessMarqant`
/// for environments where shelling out isn't desired.
pub struct KompressMarqant {
    pipeline: kompress_core::Pipeline,
}

impl KompressMarqant {
    pub fn new() -> Self {
        Self { pipeline: kompress_core::Pipeline::new() }
    }
}

impl Default for KompressMarqant {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Marqant for KompressMarqant {
    async fn compress(&self, findings: &str, _deadline: Duration) -> Result<String, PpError> {
        if findings.trim().is_empty() {
            return Ok(String::new());
        }
        let inputs: Vec<String> = findings
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        let result = self
            .pipeline
            .run(inputs)
            .map_err(|e| PpError::Marqant(format!("kompress-core pipeline failed: {e}")))?;
        Ok(result
            .units
            .into_iter()
            .map(|u| u.content)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
```

Check `crates/memory-pp/src/error.rs` for the exact `PpError::Marqant` variant shape before writing this — confirm it takes a `String` (the existing `SubprocessMarqant` impl above already constructs `PpError::Marqant(...)`; match that exact pattern).

- [ ] **Step 5: Export it**

In `crates/memory-pp/src/lib.rs:18`, change:

```rust
pub use marqant::{Marqant, StubMarqant, SubprocessMarqant};
```

to:

```rust
pub use marqant::{KompressMarqant, Marqant, StubMarqant, SubprocessMarqant};
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p entheai-memory-pp kompress_marqant -- --nocapture`
Expected: PASS (2 tests).

- [ ] **Step 7: Full crate test + clippy**

```bash
cargo test -p entheai-memory-pp
cargo clippy -p entheai-memory-pp -- -D warnings
```

Expected: all pass, clean.

- [ ] **Step 8: Commit**

```bash
git add crates/memory-pp/Cargo.toml crates/memory-pp/src/marqant.rs crates/memory-pp/src/lib.rs Cargo.lock
git commit -m "feat(memory-pp): add KompressMarqant native compressor backend"
```

---

### Task 3: Wire marqant_backend config option

**Files:**
- Modify: `crates/config/src/lib.rs` (near `PromptProcessingConfig`, ~line 893-969)
- Modify: `bin/entheai/src/main.rs:590-594`

**Interfaces:**
- Consumes: `KompressMarqant` from Task 2.
- Produces: `PromptProcessingConfig.marqant_backend: String` (default `"subprocess"`, values `"subprocess" | "stub" | "kompress"`) — preserves today's default behavior exactly (existing configs with only `marqant_cmd` set keep working).

- [ ] **Step 1: Write the failing config test**

Add to the `#[cfg(test)] mod tests` block in `crates/config/src/lib.rs` (near the existing `memory_prompt_processing_parses` test around line 622):

```rust
    #[test]
    fn marqant_backend_defaults_to_subprocess() {
        let pp = PromptProcessingConfig::default();
        assert_eq!(pp.marqant_backend, "subprocess");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-config marqant_backend_defaults_to_subprocess`
Expected: FAIL with `no field marqant_backend on type PromptProcessingConfig`.

- [ ] **Step 3: Add the field**

In `crates/config/src/lib.rs`, in the `PromptProcessingConfig` struct (around line 893-928), add after the `native_model` field:

```rust
    /// Stage-3 compressor backend: "subprocess" (default — shells out to
    /// `marqant_cmd`), "kompress" (in-process kompress-core pipeline, no
    /// subprocess), or "stub" (identity passthrough, always falls back safely).
    #[serde(default = "default_pp_marqant_backend")]
    pub marqant_backend: String,
```

In the `impl Default for PromptProcessingConfig` block (around line 930-944), add:

```rust
            marqant_backend: default_pp_marqant_backend(),
```

Add the default function near `default_pp_marqant_cmd()` (around line 964):

```rust
fn default_pp_marqant_backend() -> String {
    "subprocess".into()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p entheai-config marqant_backend_defaults_to_subprocess`
Expected: PASS.

- [ ] **Step 5: Wire the backend selection in main.rs**

In `bin/entheai/src/main.rs`, replace lines 590-594:

```rust
    let marqant: Box<dyn Marqant> = if pc.marqant_cmd.trim().is_empty() {
        Box::new(StubMarqant)
    } else {
        Box::new(SubprocessMarqant::new(&pc.marqant_cmd))
    };
```

with:

```rust
    let marqant: Box<dyn Marqant> = match pc.marqant_backend.trim() {
        "stub" => Box::new(StubMarqant),
        "kompress" => Box::new(KompressMarqant::new()),
        other => {
            if !other.is_empty() && other != "subprocess" {
                log::warn!("unknown pp marqant_backend {other:?}; using subprocess");
            }
            if pc.marqant_cmd.trim().is_empty() {
                Box::new(StubMarqant)
            } else {
                Box::new(SubprocessMarqant::new(&pc.marqant_cmd))
            }
        }
    };
```

Update the `use entheai_memory_pp::{...}` import block at line 539-542 to include `KompressMarqant`:

```rust
    use entheai_memory_pp::{
        KompressMarqant, Marqant, MeshSearch, NativeMesh, PromptProcessor, RawStore,
        RetrievalMode, SidecarMesh, StubMarqant, StubMesh, SubprocessMarqant,
    };
```

- [ ] **Step 6: Build and test**

```bash
cargo build -p entheai
cargo test -p entheai-config
```

Expected: builds clean, config tests pass.

- [ ] **Step 7: Clippy**

```bash
cargo clippy -p entheai-config -p entheai -- -D warnings
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/config/src/lib.rs bin/entheai/src/main.rs
git commit -m "feat(config): add marqant_backend option, wire KompressMarqant"
```

---

### Task 4: Compression pulse in BrainState

**Files:**
- Modify: `crates/viz/src/brain.rs`

**Interfaces:**
- Consumes: nothing new from other tasks — takes plain `f64`/`usize` stats so `crates/tui` (Task 5) can feed it without depending on `memory-pp` or `kompress-core` types.
- Produces: `BrainState.set_compression(&mut self, ratio: f64, input_tokens: usize, output_tokens: usize)`, rendered in `footer_line`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/viz/src/brain.rs` (near `nats_and_ctx_round_trip`, around line 284):

```rust
    #[test]
    fn compression_round_trip_and_footer_shows_ratio() {
        let mut b = BrainState::new();
        b.set_compression(0.42, 1000, 420);
        assert!((b.compression_ratio - 0.42).abs() < 1e-9);
        assert_eq!(b.compression_tokens, (1000, 420));

        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::symbols::Marker;
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        render(&b, area, &mut buf, Marker::Braille);
        let y = area.bottom() - 1;
        let mut row = String::new();
        for x in area.left()..area.right() {
            row.push_str(buf[(x, y)].symbol());
        }
        assert!(row.contains("kx 42%"), "footer compression readout missing: {row:?}");
    }

    #[test]
    fn zero_compression_activity_omits_readout() {
        let b = BrainState::new();
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::symbols::Marker;
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        render(&b, area, &mut buf, Marker::Braille);
        let y = area.bottom() - 1;
        let mut row = String::new();
        for x in area.left()..area.right() {
            row.push_str(buf[(x, y)].symbol());
        }
        assert!(!row.contains("kx"), "no compression activity yet, readout should be absent: {row:?}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p entheai-viz compression_round_trip`
Expected: FAIL with `no field compression_ratio on type BrainState`.

- [ ] **Step 3: Add the fields and setter**

In `crates/viz/src/brain.rs`, in the `BrainState` struct (around line 43-52), add:

```rust
    /// Last compression cycle's output/input ratio, e.g. 0.42 == kept 42% of tokens.
    /// `None`/zero-tokens state (never compressed yet) is represented by
    /// `compression_tokens == (0, 0)`, checked by `footer_line` before rendering.
    pub compression_ratio: f64,
    pub compression_tokens: (usize, usize),
```

In `BrainState::new()` (around line 59-73), add to the struct literal:

```rust
            compression_ratio: 0.0,
            compression_tokens: (0, 0),
```

After `set_ctx_pct` (around line 129), add:

```rust
    pub fn set_compression(&mut self, ratio: f64, input_tokens: usize, output_tokens: usize) {
        self.compression_ratio = ratio;
        self.compression_tokens = (input_tokens, output_tokens);
    }
```

- [ ] **Step 4: Extend footer_line**

Replace `footer_line` (around line 212-232) with:

```rust
fn footer_line(state: &BrainState) -> Line<'static> {
    let (nats_glyph, nats_col) = if state.nats_up {
        ("●", Color::Green)
    } else {
        ("○", Color::DarkGray)
    };
    let ctx_col = if state.ctx_pct >= 85 {
        Color::Red
    } else if state.ctx_pct >= 60 {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let mut spans = vec![
        Span::styled(format!("wk {}", state.worker_count), Style::default().fg(Color::Gray)),
        Span::raw(" · nats "),
        Span::styled(nats_glyph, Style::default().fg(nats_col)),
        Span::raw(" · "),
        Span::styled(format!("ctx {}%", state.ctx_pct), Style::default().fg(ctx_col)),
    ];
    if state.compression_tokens.1 > 0 || state.compression_tokens.0 > 0 {
        let pct = (state.compression_ratio * 100.0).round() as i64;
        spans.push(Span::raw(" · "));
        spans.push(Span::styled(format!("kx {pct}%"), Style::default().fg(Color::Magenta)));
    }
    Line::from(spans)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p entheai-viz`
Expected: all pass, including the two new tests and the pre-existing `render_small_buffer_no_panic_and_footer` (unaffected — it never calls `set_compression`, so the new readout stays absent there too, satisfying `zero_compression_activity_omits_readout`'s same assumption).

- [ ] **Step 6: Clippy**

```bash
cargo clippy -p entheai-viz -- -D warnings
```

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/viz/src/brain.rs
git commit -m "feat(viz): add compression pulse readout to brain panel footer"
```

---

### Task 5: Feed live compression stats into the TUI's BrainState

**Files:**
- Modify: `crates/tui/src/lib.rs` (single call site — do not touch the render/layout code from Task 4's area)

**Interfaces:**
- Consumes: `BrainState::set_compression` from Task 4.
- Consumes: wherever the TUI currently learns the outcome of a prompt-processing call for the current turn (the `run_task`/`run_task_with_memory` call site — confirm the exact function name and return shape by reading `crates/tui/src/lib.rs` around the existing `app.brain.flare(...)` call sites at lines 793/800/817/832 first, since this is a 95th-percentile churn hotspot and the exact turn-result shape must be read live, not assumed).

- [ ] **Step 1: Locate the turn-result call site**

Run: `grep -n "run_task\|PromptProcessor\|pp\.retrieve\|marqant" crates/tui/src/lib.rs`

Read the ~20 lines around each hit to find where (if anywhere) the TUI currently has access to a `PipelineResult`-shaped outcome, or whether prompt-processing runs entirely inside `crates/core` and only a summary crosses into the TUI (e.g. via the same channel that currently updates `app.brain.set_ctx_pct(...)`).

- [ ] **Step 2: Report findings before writing code**

If prompt-processing results do NOT currently cross into the TUI layer at all (likely, since PP Slice 1/2 work has focused on `crates/core`/`bin/entheai` per the memory-pp module docs — "the core call site treats as fall back to today's top-K"), this task's scope reduces to: confirm that gap explicitly, do NOT invent a new channel/field in `crates/core` speculatively, and instead stop here and hand back to the user/planner with the exact current wiring (file:line where the turn result is discarded) so the cross-crate plumbing (core → tui) can be scoped as its own follow-up task once the shape of that data is known.

This step is deliberately a checkpoint, not a code change — do not guess at `crates/core`'s internals from this plan; `crates/core/src/lib.rs` is the highest-severity hotspot in the repo (defect health 1.9/10) and any change there needs its own `get_risk` pass and a human decision on where the wire actually threads through, which is out of scope for a viz-crate plan.

- [ ] **Step 3: Commit the checkpoint note (no code)**

If Step 2 concludes the wiring doesn't exist yet, write a short note to `docs/superpowers/plans/2026-07-22-kompress-core-port.md` (this file) under a new `## Follow-up` section at the end, stating the exact file:line where turn results are currently dropped, then commit just this doc update:

```bash
git add docs/superpowers/plans/2026-07-22-kompress-core-port.md
git commit -m "docs(plan): checkpoint — core-to-tui compression wiring needs its own plan"
```

---

## Follow-up

Task 5 checkpoint result: `crates/tui/src/lib.rs:729` calls `.run_task(...)`, not
`.run_task_with_memory(...)` — confirmed by the pre-existing
`TODO(@rahulmranga)` at `crates/tui/src/lib.rs:318-323`, which already tracks
this exact gap and points to `docs/superpowers/plans/2026-07-19-entheai-memory-v1.md`
→ "Task 9" for the verbatim wiring recipe. Prompt-processing (and therefore
any `KompressMarqant`/compression-ratio data) does not reach the interactive
TUI loop at all yet — it's an `bin/entheai`-and-below concern only today.

Wiring the Task-4 `BrainState::set_compression` call therefore depends on
Task 9 landing first (threading `memory`/`pp` into `run`/`event_loop`), plus
a shape change to `PromptProcessor::retrieve` (or a new accessor) since it
currently returns `Option<String>` — the brief text only, not the
`PipelineResult` stats (`compression_ratio`, `input_tokens`, `output_tokens`)
needed to call `set_compression`. Both are `crates/core`/`crates/tui`
hotspot changes and out of scope for this plan; scope them as a follow-up
plan once Task 9 is prioritized.

## Self-Review Notes

- Tasks 1-4 are fully self-contained and independently testable/committable; Task 5 is intentionally a research-and-checkpoint task rather than a blind code change, because the exact plumbing from `crates/core` (where `PromptProcessor::retrieve` is actually called) into the TUI's `BrainState` cannot be verified without reading a hotspot file live at execution time.
- `kompress-brain` is deliberately excluded from all tasks per the user's steer: only the `buildBrainLine` *pattern* (compact liveness line, icon + counts + age) is adapted, and Task 4 does exactly that against entheai's own live state — no mygraph, no persons.rs lore, no external file paths.
