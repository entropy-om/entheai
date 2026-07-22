# Frozen Nodes (Slice 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Curated best-practice "frozen nodes" that sit dormant and unfreeze when a task's deterministic triggers match — ordered by relevance, distilled through marqant, injected as a bounded transient prompt block, and glowing in the brain panel.

**Architecture:** A new module `crates/memory-pp/src/frozen.rs` reusing the existing `SubprocessMarqant` (distill) and the crate-private `lexical_score` (deterministic ordering). A curated `frozen/*.md` store (TOML front-matter + markdown body). `BrainState` gains a frozen ring. Wired opt-in at prompt assembly. Fail-safe throughout (frozen never fails a task).

**Tech Stack:** Rust, `toml` (front-matter parse — already a workspace dep), `#[tokio::test]`, ratatui.

Spec: `docs/superpowers/specs/2026-07-22-frozen-nodes-design.md`.

---

## Task 1: `FrozenNode` + parse a single node file

**Files:**
- Create: `crates/memory-pp/src/frozen.rs`
- Modify: `crates/memory-pp/src/lib.rs` (add `pub mod frozen;` + re-exports)

- [ ] **Step 1: Write the failing test** (in `frozen.rs`'s `#[cfg(test)] mod tests`):

```rust
#[test]
fn parse_node_reads_frontmatter_and_body() {
    let raw = "+++\nname = \"nixos\"\ndomain = \"cloud\"\ntriggers = [\"hetzner\",\"ssh\"]\nmcp = \"nixos\"\nrank = 1.0\n+++\nPrefer NixOS for deploys.\n";
    let n = FrozenNode::parse(raw).expect("parses");
    assert_eq!(n.name, "nixos");
    assert_eq!(n.triggers, vec!["hetzner", "ssh"]);
    assert_eq!(n.mcp.as_deref(), Some("nixos"));
    assert_eq!(n.rank, 1.0);
    assert_eq!(n.knowledge.trim(), "Prefer NixOS for deploys.");
    // a file without the +++ fences, or with no name, is None (skipped, not a panic)
    assert!(FrozenNode::parse("no frontmatter here").is_none());
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-memory-pp parse_node_reads` → FAIL (no `FrozenNode`).
- [ ] **Step 3: Implement**

```rust
//! Frozen nodes — curated best-practice that wakes on deterministic triggers.
//! See docs/superpowers/specs/2026-07-22-frozen-nodes-design.md.

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct FrozenNode {
    pub name: String,
    pub domain: String,
    pub triggers: Vec<String>,
    pub mcp: Option<String>,
    pub rank: f32,
    pub knowledge: String,
}

#[derive(Debug, Deserialize)]
struct FrontMatter {
    name: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    mcp: Option<String>,
    #[serde(default = "default_rank")]
    rank: f32,
}
fn default_rank() -> f32 { 1.0 }

impl FrozenNode {
    /// Parse a `+++`-fenced TOML front-matter + markdown body. Returns None for a
    /// malformed file (caller skips it) — never panics.
    pub fn parse(raw: &str) -> Option<FrozenNode> {
        let rest = raw.strip_prefix("+++")?;
        let end = rest.find("+++")?;
        let fm: FrontMatter = toml::from_str(rest[..end].trim()).ok()?;
        let knowledge = rest[end + 3..].trim().to_string();
        Some(FrozenNode {
            name: fm.name,
            domain: fm.domain,
            triggers: fm.triggers,
            mcp: fm.mcp,
            rank: fm.rank,
            knowledge,
        })
    }
}
```

Add `toml = { workspace = true }` to `crates/memory-pp/Cargo.toml` `[dependencies]`, and `pub mod frozen;` + `pub use frozen::{FrozenNode, FrozenStore};` to `lib.rs`.

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-memory-pp parse_node_reads` → PASS.
- [ ] **Step 5: Commit** — `git add crates/memory-pp/ && git commit -m "feat(memory-pp): FrozenNode front-matter parse"`

## Task 2: `FrozenStore::load` — a directory of nodes, malformed skipped

**Files:**
- Modify: `crates/memory-pp/src/frozen.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn store_loads_dir_and_skips_malformed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("nixos.md"),
        "+++\nname=\"nixos\"\ntriggers=[\"hetzner\"]\n+++\nuse nix").unwrap();
    std::fs::write(dir.path().join("broken.md"), "garbage, no frontmatter").unwrap();
    let store = FrozenStore::load(dir.path());
    assert_eq!(store.len(), 1, "malformed file skipped, the good one loads");
    assert_eq!(store.nodes()[0].name, "nixos");
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-memory-pp store_loads_dir` → FAIL.
- [ ] **Step 3: Implement**

```rust
pub struct FrozenStore {
    nodes: Vec<FrozenNode>,
}

impl FrozenStore {
    /// Load every `*.md` in `dir`; skip (warn) any that don't parse. A missing dir
    /// yields an empty store (frozen simply never wakes) — never an error.
    pub fn load(dir: &std::path::Path) -> FrozenStore {
        let mut nodes = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return FrozenStore { nodes };
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            match std::fs::read_to_string(&p).ok().and_then(|raw| FrozenNode::parse(&raw)) {
                Some(n) => nodes.push(n),
                None => log::warn!("frozen: skipping malformed node {}", p.display()),
            }
        }
        FrozenStore { nodes }
    }
    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }
    pub fn nodes(&self) -> &[FrozenNode] { &self.nodes }
}
```

- [ ] **Step 4: Run, verify pass** — PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(memory-pp): FrozenStore::load (skip malformed)"`

## Task 3: `wake` — deterministic trigger match + relevance ordering

**Files:**
- Modify: `crates/memory-pp/src/frozen.rs`; make `lexical_score` reachable — in `crates/memory-pp/src/mesh.rs` change `fn lexical_score` to `pub(crate) fn lexical_score`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn wake_matches_triggers_and_orders_by_relevance() {
    let nodes = vec![
        FrozenNode { name: "nixos".into(), domain: "cloud".into(),
            triggers: vec!["hetzner".into(), "deploy".into()], mcp: None, rank: 1.0,
            knowledge: "nixos reproducible deploy to hetzner via ssh".into() },
        FrozenNode { name: "ngrok".into(), domain: "tunnels".into(),
            triggers: vec!["ngrok".into()], mcp: None, rank: 1.0,
            knowledge: "ngrok quick tunnel".into() },
    ];
    let store = FrozenStore::from_nodes(nodes);
    let woken = store.wake("please deploy the service to hetzner", 1);
    assert_eq!(woken.len(), 1);
    assert_eq!(woken[0].name, "nixos", "trigger match + relevance picks nixos");
    assert!(store.wake("unrelated task about cats", 1).is_empty(), "no trigger → no wake");
}
```

- [ ] **Step 2: Run, verify fail** — FAIL (`from_nodes`/`wake` missing).
- [ ] **Step 3: Implement** — add `from_nodes` (test ctor) + `wake`:

```rust
impl FrozenStore {
    pub fn from_nodes(nodes: Vec<FrozenNode>) -> FrozenStore { FrozenStore { nodes } }

    /// Deterministic trigger match → candidates, ordered by lexical relevance of the
    /// prompt to each node's knowledge plus its `rank` prior; best first, ≤ `top_k`.
    pub fn wake(&self, prompt: &str, top_k: usize) -> Vec<FrozenNode> {
        let p = prompt.to_lowercase();
        let mut cands: Vec<&FrozenNode> = self
            .nodes
            .iter()
            .filter(|n| n.triggers.iter().any(|t| trigger_hit(&p, &t.to_lowercase())))
            .collect();
        cands.sort_by(|a, b| {
            let sa = crate::mesh::lexical_score(prompt, &a.knowledge) + 0.25 * a.rank;
            let sb = crate::mesh::lexical_score(prompt, &b.knowledge) + 0.25 * b.rank;
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        cands.into_iter().take(top_k).cloned().collect()
    }
}

/// A trigger matches if it's a substring of the (lowercased) prompt; a trailing `*`
/// makes it a prefix-glob on whitespace-delimited words.
fn trigger_hit(prompt_lc: &str, trigger_lc: &str) -> bool {
    if let Some(prefix) = trigger_lc.strip_suffix('*') {
        prompt_lc.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(prefix))
    } else {
        prompt_lc.contains(trigger_lc)
    }
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-memory-pp wake_matches` → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(memory-pp): FrozenStore::wake (deterministic triggers + relevance order)"`

## Task 4: `activate` — marqant-distilled, bounded, fail-safe

**Files:**
- Modify: `crates/memory-pp/src/frozen.rs`

- [ ] **Step 1: Failing test** (reuse the fake-`mq` pattern from `marqant.rs` tests):

```rust
#[tokio::test]
async fn activate_distills_then_caps() {
    use crate::marqant::{Marqant, StubMarqant};
    let node = FrozenNode { name: "nixos".into(), domain: "cloud".into(),
        triggers: vec![], mcp: None, rank: 1.0, knowledge: "use nix flakes for pinned inputs".into() };
    // StubMarqant is identity → the brief carries the knowledge, size-capped, tagged.
    let brief = activate(&node, &StubMarqant, 4096, std::time::Duration::from_millis(50)).await;
    assert!(brief.contains("frozen:nixos"), "brief is tagged: {brief}");
    assert!(brief.contains("nix flakes"), "brief carries the knowledge");
    // a tiny cap truncates
    let short = activate(&node, &StubMarqant, 12, std::time::Duration::from_millis(50)).await;
    assert!(short.len() <= 64, "respects the byte cap (+ tag): {}", short.len());
}
```

- [ ] **Step 2: Run, verify fail** — FAIL (`activate` missing).
- [ ] **Step 3: Implement**

```rust
use crate::marqant::Marqant;
use std::time::Duration;

/// Distil a woken node's knowledge through `mq` (fail-safe: raw on error), cap it, tag it.
/// The returned brief is meant to be injected transiently — NEVER persisted.
pub async fn activate(
    node: &FrozenNode,
    marqant: &dyn Marqant,
    max_bytes: usize,
    deadline: Duration,
) -> String {
    let body = match marqant.compress(&node.knowledge, deadline).await {
        Ok(b) if !b.trim().is_empty() => b,
        _ => node.knowledge.clone(), // mq missing/slow/empty → raw (never blocks)
    };
    let capped = cap_bytes(&body, max_bytes);
    format!("❄→☀ frozen:{} — {}", node.name, capped)
}

fn cap_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) { end -= 1; }
    &s[..end]
}
```

- [ ] **Step 4: Run, verify pass** — PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(memory-pp): frozen activate (marqant distill + cap, fail-safe)"`

## Task 5: Brain frozen ring (`crates/viz`)

**Files:**
- Modify: `crates/viz/src/brain.rs` (BrainState + render)

- [ ] **Step 1: Failing test** (in brain.rs tests):

```rust
#[test]
fn frozen_node_wakes_and_melts() {
    let mut b = BrainState::new();
    b.set_frozen(&["nixos".to_string(), "ngrok".to_string()]);
    assert_eq!(b.frozen_awake("nixos"), 0.0, "starts frozen");
    b.wake_frozen("nixos");
    assert_eq!(b.frozen_awake("nixos"), 1.0, "wakes fully");
    for _ in 0..200 { b.tick(); }
    assert!(b.frozen_awake("nixos") < 0.02, "melts back toward frozen");
}
```

- [ ] **Step 2: Run, verify fail** — FAIL.
- [ ] **Step 3: Implement** — add a `frozen: Vec<FrozenGlow>` field (`struct FrozenGlow { name: String, awake: f32 }`) to `BrainState`; `set_frozen(&[String])` builds them at `awake=0`; `wake_frozen(name)` sets that node's `awake=1.0`; `frozen_awake(name)->f32`; extend `tick()` to decay each `frozen[i].awake *= DECAY`. In `render`, draw the frozen nodes as an outermost ring (radius ~1.1 projected, or reuse the fleet-ring math) with brightness = `awake` (dim floor so frozen nodes are faintly visible). Update any `BrainState { .. }` literals if the struct is constructed positionally (it uses `::new()` — check tests).

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-viz frozen_node_wakes`; `cargo clippy -p entheai-viz --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit** — `git commit -am "feat(viz): brain frozen ring — wake + melt"`

## Task 6: Config `[frozen]`

**Files:**
- Modify: `crates/config/src/lib.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn frozen_config_defaults_off() {
    let cfg = Config::from_toml_str("").unwrap();
    assert!(!cfg.frozen.enabled);
    assert_eq!(cfg.frozen.dir, "frozen");
    assert_eq!(cfg.frozen.top_k, 1);
    let on = Config::from_toml_str("[frozen]\nenabled = true\ntop_k = 2\n").unwrap();
    assert!(on.frozen.enabled);
    assert_eq!(on.frozen.top_k, 2);
}
```

- [ ] **Step 2: Run, verify fail** — FAIL.
- [ ] **Step 3: Implement** — add `#[serde(default)] pub frozen: FrozenConfig` to `Config`; define `FrozenConfig { #[serde(default)] enabled: bool, #[serde(default = "default_frozen_dir")] dir: String, #[serde(default = "default_frozen_top_k")] top_k: usize, #[serde(default = "default_frozen_max_bytes")] max_inject_bytes: usize }` with `impl Default` + the `default_frozen_*` fns (`"frozen"`, `1`, `4096`). Follow the existing `PromptProcessingConfig` pattern verbatim.

- [ ] **Step 4: Run, verify pass** — PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(config): [frozen] enabled/dir/top_k/max_inject_bytes (default off)"`

## Task 7: Seed the `frozen/` store

**Files:**
- Create: `frozen/nixos.md`, `frozen/github.md`, `frozen/rust.md`, `frozen/go-parallelism.md`, `frozen/python-jit.md`, `frozen/ngrok.md`, `frozen/valyu.md`

- [ ] **Step 1: Write the 7 nodes** — each a `+++` front-matter (name, domain, triggers, mcp, rank=1.0) + a concise body of the best-practice, from these seeds (tune wording):
  - **nixos** — triggers `["nixos","hetzner","ssh","deploy","reproducible","nixops","flake"]`, mcp `"nixos"`; body: prefer NixOS for reproducible cloud/deploy; flakes for pinned inputs; `nixos-rebuild switch --target-host … --use-remote-sudo`; rollbacks; gotchas.
  - **github** — triggers `["git","github","pr","pull request","commit","branch"]`; body: source-control conventions, PR flow.
  - **rust** — triggers `["rust","cargo","crate"]`; body: backend in Rust; error-handling (thiserror lib / anyhow bin); test-first.
  - **go-parallelism** — triggers `["goroutine","concurrency","parallel","channel","go "]`, mcp none; body: Go for beautiful quick parallelism; goroutines + channels; `errgroup`.
  - **python-jit** — triggers `["python","script","jit","pypy"]`; body: Python (+JIT/PyPy) for long-running quick scripts.
  - **ngrok** — triggers `["ngrok","tunnel","expose","webhook","one-off"]`, mcp none; body: ngrok for quick one-off site/webhook deploys when a devbox exists.
  - **valyu** — triggers `["research","deep research","cite","sources","valyu"]`, mcp `"valyu"`; body: use Valyu MCP for deep, cited research.

- [ ] **Step 2: Verify they load** — a quick test or `store_loads_dir`-style assert that `FrozenStore::load("frozen")` returns 7 nodes.
- [ ] **Step 3: Commit** — `git add frozen/ && git commit -m "feat(frozen): seed 7 curated nodes"`

## Task 8: Wire at prompt assembly + docs

**Files:**
- Modify: the prompt-assembly path — build the store once at startup (bin/TUI), and where the user prompt is assembled, when `cfg.frozen.enabled`: `store.wake(prompt, top_k)` → for the top node `activate(node, marqant, max_bytes, deadline)` → inject the brief as a system/context message before the last user turn (mirror how PP's brief is injected) → `app.brain.wake_frozen(&node.name)` (TUI) + `brain.set_frozen(names)` at init. Best-effort: log + skip on any error.
- Modify: `entheai.toml` (document `[frozen]`), `CHANGELOG.md`.

- [ ] **Step 1: Wire it** — reuse the `SubprocessMarqant`/`StubMarqant` already built for PP (respect `[memory.prompt_processing] marqant_cmd`, or a `[frozen]` marqant if you prefer; default to the identity stub when unset). Independent of `[memory] mode`.
- [ ] **Step 2: Build + test** — `cargo test --workspace`; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] **Step 3: Manual** — launch the TUI with `[frozen] enabled = true`, prompt "deploy to hetzner", confirm the `❄→☀ frozen:nixos` brief is injected and the brain node glows.
- [ ] **Step 4: Commit** — `git commit -am "feat: wire frozen-node enrichment at prompt assembly + docs"`

---

## Final verification
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo test --workspace` green
- [ ] Frozen disabled → prompt assembly byte-identical to today

## Self-review notes
- **Spec coverage:** model+store (T1–T2), wake/triggers/order (T3), marqant distill+cap+fail-safe (T4), brain glow (T5), config (T6), seed (T7), wiring+docs (T8) — all spec sections mapped.
- **Type consistency:** `FrozenNode{name,domain,triggers,mcp,rank,knowledge}`, `FrozenStore::{load,from_nodes,wake,nodes,len}`, `activate(&FrozenNode,&dyn Marqant,usize,Duration)->String`, `BrainState::{set_frozen,wake_frozen,frozen_awake}` used consistently.
- **Deferred (not in this plan):** MCP auto-load (Docker MCP), experience-fed re-ranking + mining, external sources — Slices 2–4.
