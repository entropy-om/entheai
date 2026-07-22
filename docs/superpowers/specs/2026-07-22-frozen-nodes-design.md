# Frozen Nodes — design (Slice 1)

## The idea

A **frozen node** is a curated unit of *domain best-practice* — a preference plus its
patterns, gotchas, and the right tool/MCP for a problem space (NixOS for reproducible
cloud/deploy, GitHub for source control, Go for beautiful parallelism, Python+JIT for
long quick scripts, ngrok for one-off deploys, Valyu for deep research). It sits
**dormant** by default. When a task matches its problem space, it **unfreezes**: its
knowledge wakes into the prompt (bounded), and it glows in the brain.

**The ice-in-coca-cola property.** Waking a node doesn't overflow context. Only the top
node's *distilled* brief melts into the prompt, size-capped, transient — never persisted.
The raw node stays frozen in the store ("always there, like ice"); when the task passes
it re-freezes. No cross-turn accumulation → "the drink won't spill, just a bit more
watery, colder." Determinism over randomness: the *wake* is a deterministic trigger match.

**Evolvable, not static.** Frozen nodes are collected and re-ranked — the *ordering* of
which best-practice is current for a problem space evolves via the reranker. (Automatic
experience-fed rank updates + node mining are Slice 2.)

## Scope

This spec is **Slice 1**: the node model + store + deterministic-trigger wake + reranker
ordering + marqant-distilled bounded injection + brain glow. Explicitly deferred:
- **Slice 2** — MCP auto-load on wake via the Docker MCP interface (NixOS MCP, Valyu, …).
- **Slice 3** — automatic experience-fed re-ranking (bump a node's rank when its woken
  task succeeded) + mining new nodes from the raw store.
- **Slice 4** — external raw-context sources (WorldMonitor, Wikipedia) feeding the raw
  store; Valyu wired as a frozen-node deep-research MCP.

## Home

Folds into **`crates/memory-pp`** as a new module `frozen.rs` — it reuses what's already
there: the ultragraph reranker (`NativeMesh` scoring / `entheai-ultragraph`) for ordering
and `SubprocessMarqant` for distillation. No new crate.

## Architecture

### 1. The node model + curated store

```rust
pub struct FrozenNode {
    pub name: String,              // "nixos"
    pub domain: String,            // "reproducible cloud / system builds"
    pub triggers: Vec<String>,     // ["hetzner","ssh","deploy","nixos","reproducible"]
    pub mcp: Option<String>,       // associated MCP (used by the deferred Slice-2 auto-load)
    pub rank: f32,                 // curated prior; experience-updated in Slice 3
    pub knowledge: String,         // the distilled best-practice (patterns · gotchas · prefs)
}
```

Stored one-node-per-file: `frozen/<name>.md`, TOML front-matter + markdown body:

```markdown
+++
name = "nixos"
domain = "reproducible cloud / system builds"
triggers = ["hetzner", "ssh", "deploy", "nixos", "reproducible", "nixops"]
mcp = "nixos"
rank = 1.0
+++
Prefer NixOS for any cloud setup/deploy: declarative, reproducible, rollback-safe.
Patterns: flakes for pinned inputs; `nixos-rebuild switch --target-host` over SSH; …
Gotchas: …
```

`FrozenStore::load(dir)` reads every `*.md`, parses front-matter + body, skips a malformed
node with a `warn!` (fail-safe — one bad file never breaks the set). The repo ships a
seeded `frozen/` dir: `nixos`, `github`, `rust`, `go-parallelism`, `python-jit`, `ngrok`,
`valyu`. Config `[frozen] dir` (default `"frozen"`), resolved from the run root.

### 2. Wake — deterministic trigger + reranker ordering

```rust
pub async fn wake(&self, prompt: &str) -> Vec<Woken>;   // ordered, best first, ≤ top_k
```

1. **Deterministic match** (no model call): lowercase the prompt; a node is a *candidate*
   iff any `trigger` matches (word / substring / simple `*` glob). Inspectable, reproducible.
2. **Rerank**: order candidates by `score(prompt, node.knowledge)` — the same scorer
   `NativeMesh` uses (the `.ugm` reranker when configured, else the lexical fallback) —
   combined with the `rank` prior (`final = reranker_score + w·rank`). Take the top `top_k`
   (default 1; `[frozen] top_k`).

Empty candidate set → no wake (byte-identical to frozen-off).

### 3. Activation — marqant-distilled, bounded, transient

For the top woken node(s):
1. **Distill** `node.knowledge` through `SubprocessMarqant` (the PP Stage-3 compressor) to
   a tight brief. **Fail-safe:** if `mq` is missing/errors/times out, fall back to the raw
   knowledge capped to a byte budget (`[frozen] max_inject_bytes`, default 4 KiB) — never
   block, mirroring PP's fallback-first discipline.
2. **Inject** the brief as a bounded context block immediately before the last user turn:
   `❄→☀ frozen:nixos — <brief>`. Only the top node, size-capped.
3. **Transient — the melt.** The brief is injected for *this task only*; it is **never**
   written to memory, the raw store, or carried to the next turn. The raw node stays in the
   store; the brief re-freezes when the task passes. No accumulation.

Independent of `[memory] mode` — frozen enrichment runs at prompt assembly whether memory
is `topk` or `prompt-processing`. Opt-in: `[frozen] enabled` (default false in Slice 1).

### 4. Brain glow

`BrainState` (crates/viz) gains a **frozen ring**: one node per frozen node, dim (frozen)
by default. A `wake` flares the matching node's `awake` to `1.0`; the existing per-tick
decay eases it back down — you watch the ice melt and re-freeze. Pure model + a mutator
`brain.wake_frozen(name)`; the TUI calls it from the wake path. Renders as an outer ring in
`brain.rs`, brightness = `awake`, labeled by node name (first char).

### 5. Wiring

At prompt assembly (core `run_task*` / the TUI submit path), when `[frozen] enabled`:
`store.wake(prompt)` → distill+inject the top node's brief → `brain.wake_frozen(name)`.
Best-effort: any error logs and skips (frozen never fails a task).

## Error handling & fail-safes

- Malformed node file → skipped with a warn; the rest load.
- `mq` absent/slow → raw capped injection (never blocks).
- No trigger match → no injection (identical to frozen-off).
- Empty/oversized knowledge → capped; empty brief → no injection.
- Frozen disabled → zero overhead, byte-identical to today.

## Testing

- **Store:** load a temp `frozen/` dir; front-matter + body parse; a malformed file is
  skipped, the rest load.
- **Wake — deterministic:** a prompt containing "deploy to hetzner via ssh" wakes `nixos`;
  an unrelated prompt wakes nothing. Case-insensitive; glob triggers.
- **Wake — ordering:** two candidates → the reranker orders the more relevant first;
  `rank` breaks near-ties.
- **Activation:** the injected brief is present, size-capped, and (assert) not written to
  any store; `mq`-absent falls back to raw-capped (via a fake-`mq`, reusing the marqant
  test harness).
- **Brain:** `wake_frozen(name)` flares that node to 1.0; `tick` decays it toward 0.
- **Fail-safe:** frozen disabled → prompt assembly is byte-identical.

## File structure

- **Create** `crates/memory-pp/src/frozen.rs` — `FrozenNode`, `FrozenStore`, `wake`,
  `activate` (distill+cap). Re-export from `memory-pp/src/lib.rs`.
- **Create** `frozen/*.md` — the 7 seeded nodes.
- **Modify** `crates/viz/src/brain.rs` — the frozen ring + `wake_frozen`.
- **Modify** `crates/config/src/lib.rs` — `[frozen] enabled`/`dir`/`top_k`/`max_inject_bytes`.
- **Modify** the wire point (core/TUI) — call `wake` + inject + brain flare at prompt assembly.
- **Docs** — `entheai.toml` + CHANGELOG.

## Scope check

One subsystem (a curated enrichment layer) folded into memory-pp, reusing the reranker and
marqant. Slice 1 is a complete, testable increment (wake → distill → inject → glow); the MCP
auto-load, experience-fed evolution, and external sources are sequenced as Slices 2–4.
