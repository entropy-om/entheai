# entheai — Rust Style & Performance Guide

> Our own tips, tricks, and performance rules. Living document. Held to at code review.
> Canonical sources: David Lattimore, [*Wild performance tricks*](https://davidlattimore.github.io/posts/2025/09/02/rustforge-wild-performance-tricks.html) · [vandadnp/rust-tips-and-tricks](https://github.com/vandadnp/rust-tips-and-tricks).

entheai is a terminal coding-agent whose hot paths run **per streamed token, per render frame, per agent turn, and per fan-out sub-agent**. Correctness and idiom matter everywhere; **performance discipline matters most on those hot paths**. This guide is the standard a reviewer holds a PR to. When a rule and a measurement disagree, the measurement wins — but know the rule first.

---

## Part 1 — Hot-path performance rules

### P1. Know your hot path's complexity — and keep it O(1) or O(Δ)
Code that runs *per token / per frame / per tick* must cost a **constant** amount, or an amount proportional to **what changed (Δ)** — never proportional to the **total** accumulated so far. An O(n) step on a per-token path is O(n²) per turn.

- **Anti-pattern (real, `crates/tui`):** the render `LineCache` keys on `(msg_count, last_msg_len, width)`, so appending each streamed token invalidates it and re-wraps the **entire scrollback** every token → **O(history) per token = O(n²) per turn**. Fix: cache wrapped lines **per message**; only re-wrap the one streaming message → O(Δ) per token.
- **Rule:** for any function on a per-frame/token/tick path, write its complexity in a comment and justify why it's not O(total).

### P2. Don't allocate in hot loops
Per-token/per-frame allocation is the most common self-inflicted cost.

- Build strings with `write!(&mut buf, …)` (reusing/`clear()`-ing a `String`), **not** `s.push_str(&format!(…))` — the latter allocates a throwaway `String` each call (clippy `format_push_string`). We do this in `crates/obsidian/generators.rs`; acceptable there (per-tick, not per-token) but never on a per-token path.
- `reserve`/`with_capacity` when the final size is known; avoid `Vec::new()` that immediately grows.
- Avoid `.collect()` then iterate — chain iterators and consume once.
- Avoid `.clone()` to satisfy the borrow checker on a hot path; restructure the borrow or move the clone out of the loop. A clone of a large struct (message history, config) per turn is a smell.

### P3. Make caches that actually hit
A cache whose key changes every call is pure overhead (see P1). The key must be **stable across the calls you want to hit** and invalidate only on real change. Prefer content-independent keys (indices, versions) over "last length"-style keys that churn.

### P4. O(n) → O(1): index instead of scan
A linear `find`/`position`/`contains` on a hot path over a growing collection is O(n); a `HashMap`/`HashSet`/precomputed index is O(1). For **hot, trusted-key** maps prefer a fast hasher (`rustc_hash::FxHashMap` / `ahash`) over the std default SipHash (SipHash is DoS-resistant but slower; we don't need DoS-resistance for internal keys). Keep SipHash for maps keyed by untrusted/external input.

### P5. Bound everything driven by external input or a long life
Unbounded growth is a latent OOM/DoS.

- **External input:** cap allocations driven by model/provider/tool/MCP output. Real fixes already in tree: `crates/providers` caps a provider-controlled tool-call `index` (`MAX_TOOL_CALLS`); MCP/shell output must be read through a **bounded** reader, not buffered-then-truncated.
- **Long-lived structures:** reap them. `crates/orchestrator`'s `WorkerPool` inserts per spawn and never removes → unbounded across fan-out runs. A per-session/per-turn collection needs a removal path.

### P6. Keep blocking work off the async runtime
Tokio worker threads are for *await*-ing, not for CPU/FS grinding.

- Wrap blocking FS/CPU in `tokio::task::spawn_blocking` (or a dedicated thread). `crates/obsidian`'s `apply()` (full `walkdir` + `read_to_string` + writes) must not run inline on a worker.
- **Never hold a lock across `.await`.** Take the guard inside a scope that ends before the await (the memory store does this correctly: `Mutex<Connection>` is locked *inside* each `spawn_blocking` closure). A single lock that serializes all hot access is a throughput ceiling — know when you've built one.

### P7. Prefer static dispatch on hot paths
Generic/`impl Trait` (monomorphized, inlinable) beats `Box<dyn Trait>` (vtable, no inlining) where it runs hot. Use `dyn` at boundaries (plugin/tool registries, trait objects stored in a `Vec`) where the flexibility is worth the indirection — not in the inner loop.

### P8. Measure before (and after) you optimize
The workspace release profile is already tuned: `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `mimalloc` global allocator on macOS. Before micro-optimizing, confirm the path is actually hot (`ENTHEAI_LOG=debug`, a `criterion` bench, or a timing log). Land the algorithmic win (Part 1) before the constant-factor win.

---

## Part 2 — Idiomatic craft

- **Struct-update syntax:** `Foo { field, ..Default::default() }` — used throughout (`ChatMessage`, config defaults). Prefer it over field-by-field.
- **`let-else`** for early-return unwrapping; **`map_or`/`map_or_else`** over `map(...).unwrap_or(...)`; **`matches!`** over a `match` returning bool; **`Self`** inside impls.
- **Iterator combinators + `?`-propagation** over manual loops and nested `match` where they read clearly.
- **Error handling convention:** `thiserror` for typed errors in **library crates**; `anyhow` in the **binary**. Don't return `anyhow::Result` from a library's public API where a typed error belongs. (Dead `anyhow`/`serde` deps were removed workspace-wide in `f8ec724` — don't re-add a dep you only *might* use.)
- **Borrow in signatures:** take `&str`/`&[T]`/`impl AsRef<Path>`/`Cow<'_, str>` rather than forcing an owned `String`/`Vec`/`PathBuf` on the caller. Return owned only when you produce it.
- **Determinism where it feeds a hash/cache:** no wall-clock/RNG in a pure render/hash path (the obsidian render layer is deterministic so the writer's content-hash is stable — see P3).

---

## Part 3 — entheai hot paths (the ones to guard)

| Hot path | Runs | Watch for |
|---|---|---|
| agent loop — `crates/core` `run_task*` | per turn | message-vec clones, memory-context assembly cost, tool-dispatch lookups |
| TUI render — `crates/tui` `event_loop`/draw | **per frame / per token** | history re-wrap (P1), per-token allocations (P2), cache churn (P3) |
| memory recall — `crates/memory` `search_hybrid` | per retrieval | over-fetch size, the single `Mutex<Connection>` serialization (P6), id-load batching |
| fan-out — `crates/orchestrator` `run_fanout`/`WorkerPool` | per sub-agent | pool reaping (P5), worktree lifecycle, `buffer_unordered` bound |
| obsidian sync — `crates/obsidian` `apply()` | **per debounced FS tick** | blocking-on-runtime (P6), full re-scan vs incremental (P1) |
| companion beacon — `crates/companion` render | **per animation frame** | per-frame recompute, correct frame-delta clock, allocation per frame |

### Hot-path findings — complexity audit (2026-07-20)

| # | Path · runs | Was | Fix → | Status |
|---|---|---|---|---|
| 1 | tui `LineCache::get_or_build` (`lib.rs:1071`) · **per token** | O(n²)/turn — key `(len, last_msg_len, width)` churns every token → re-wraps the whole scrollback | cache lines **per message** keyed by `(msg_idx, text_len, width)`; re-wrap only the streaming msg → O(Δ)/token | fixing |
| 2 | tui draw `lines.to_vec()` (`lib.rs:411`) · **per frame** | deep-clones the whole wrapped history every draw (borrow-checker appeasement) — compounds #1 | render the `Paragraph` from a borrowed `&[Line]` → O(1) extra | fixing |
| 3 | companion glow loop (`render.rs:167`) · **per pixel/frame** | `sqrt` + `smooth_falloff` per pixel every frame — invariant for the fixed 180×180 window | precompute a `glow_factor` LUT once per (W,H); per frame = `LUT[i]·pulse` + 3 `lerp_u8` | fixing |
| 4 | obsidian `apply()` (`lib.rs:111`) · **per FS tick** | re-scans + re-renders the WHOLE repo each tick; discards the debouncer's changed-path set | incremental: thread changed paths / mtime-cache `SourceDoc`s; rebuild aggregate notes only on set change | **deferred (large)** — debounced ~500ms so lower-frequency; `spawn_blocking` keeps it off the runtime meanwhile |
| 5 | companion `name.to_uppercase()` (`render.rs:240`) · **per frame** | heap-allocs a `String` every frame (`glyph_3x5` re-uppercases anyway) | uppercase once at startup / drop it | fixing |
| 6 | core `stream_turn` (`lib.rs:91`) · **per turn** | `schemas.to_vec()` clones every tool schema each turn (invariant); `messages.to_vec()` O(n²)/task | `Arc<[Value]>` schemas → O(1) clone/turn (borrow-in-trait = larger follow-up) | fixing |

### Patterns observed (fold into review)

**Bad (found + being fixed):** cache key built from a hot-path-mutating value (P3, #1) · `.to_vec()`/`.clone()` to appease the borrow checker on a per-frame/token path (P2, #1/#2/#6) · recomputing frame-invariant work in a per-pixel/frame loop (P2, #3/#5) · `push_str(&format!(…))` in string-building loops (P2 — `obsidian/generators.rs`) · discarding an event's "what changed" payload then doing full work (P1, #4).

**Good (keep doing):** `OnceLock`-cached compiled `Regex` (`generators.rs:12`) · a dirty-flag redraw gate that only draws on change (`tui:388`) + a reused softbuffer `Context`/`Surface` across frames (`companion/main.rs:196`) + `String::with_capacity(len)` in the string builders · the FNV-1a content-hash manifest that skips unchanged *writes* (`writer.rs:56`).

---

## Part 4 — Reviewer checklist (hot-path PRs)

1. Does any new per-frame/token/turn/tick code do work proportional to the **total** accumulated (not Δ)? → P1.
2. Any `format!`/`to_string`/`clone`/`collect`/`Vec::new` **inside** a hot loop? → P2.
3. Any cache whose key invalidates every call? → P3.
4. Any linear scan over a growing collection on a hot path? → P4.
5. Anything allocating from **external input** without a cap, or a long-lived collection with no reap? → P5.
6. Any blocking FS/CPU inline on a Tokio worker, or a lock held across `.await`? → P6.
7. Idioms (Part 2): struct-update, `let-else`/`map_or`, borrow-in-signature, error-type convention?

---

*Add new tricks + sources here as we designate them. Every entry should be actionable and, ideally, cite a real entheai hot path.*
