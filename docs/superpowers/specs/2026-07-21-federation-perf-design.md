# Federation Performance — Shared Base, Precompute Cache, Faster Loop

**Status:** Approved shape (2026-07-21), ready to spec-review → plan.
**Relation:** builds on F2.1/F2.2/F2.3 (distributed swarm + worker confinement). Calls this **F2.4 (perf)**; the cache slice overlaps the planned **F3 (shared state)**.

## What we're trying to do

Make each federation worker do less redundant work per coder, use less memory, and finish a bit faster — without changing what any coder actually produces. Three independent improvements plus one rule that governs all of them.

Be honest about where the win is. A coder task is usually **dominated by the time spent waiting on the model** (many network round-trips to the provider), so filesystem and cache work is a small slice of the wall clock. That means:

- The **memory win** (many coders sharing one copy of the base repo) is the most reliable benefit, and it grows with **repo size × how many coders run on a worker at once**. Big repos at high fan-out win a lot; a tiny repo with one coder wins nothing.
- The **time win** is mostly "stop re-doing setup" — skip re-downloading and re-cloning the base, skip re-computing the base's embeddings and search index.
- The **model-latency win** is real but bounded: we can cut the *number* of round-trips and shave connection overhead, but we can't make the model itself faster.

So this is a "minimal but meaningful specifically for large repos at high fan-out" effort — which is exactly the federation case.

## The one rule: every optimization is disposable

All three improvements are pure optimizations. None of them may ever slow down or break a coder. Concretely, three requirements that apply to each slice:

- **Tight deadlines.** Every optimized step has a short time budget — roughly a second or two for a filesystem step, well under a second (say 50–150 ms) for a cache lookup. A cache that's slower than just recomputing is worse than no cache.
- **Instant fallback, never fail the task.** If a step is slow, fails, or the shared thing looks corrupt, we drop it immediately and take the plain path — a full clone, a local recompute, serial tools. A coder never fails because a cache or an overlay hiccuped. This is the same "if federation is unreachable, run locally" reflex the whole system already has.
- **Loud and structured, back to the orchestrator.** When the fast path is skipped we say so, in a way the orchestrator and the `/fleet` view can see: tag each coder as *base-hit*, *miss*, or *degraded (with a reason)*. Quiet misses are counted, not shouted. But **systematic** failure — a base that's repeatedly uncacheable, a cache that's consistently timing out — escalates loudly, so the fast path can't silently rot. Fail-fast about the optimization; loud about the orchestrator knowing.

These ride on machinery that already exists: the `FanoutEvent` bus for the tags, and the `WorkResult` status for per-task outcomes.

---

## Slice 1 — Share the base repo instead of copying it per coder

**The problem.** Today, for every coder, the worker downloads the base as a git bundle and does a full `git clone` of it into a fresh temp directory (`materialize_from_bundle` in `crates/federation/src/repo.rs`). Ten coders on one base means ten downloads, ten full clones, ten separate copies of the same files in memory.

**The idea.** Unpack the base **once** and give each coder a cheap, throwaway view of it. Two levels, ship the first, keep the second as a follow-up:

- **Minimal (git worktrees).** Materialize the bundle once into a **shared bare repository**, keyed by the base commit. Then give each coder a worktree off it (`git worktree add`). Worktrees share the repository's object store (the bulk of a repo's weight) and the checkout is cheap, and it needs no special privileges. This is exactly how *local* fan-out already works, so it's a small, familiar change.
- **Bigger memory win (copy-on-write filesystem).** Layer an overlay (overlayfs, or a btrfs/reflink snapshot) with the shared base as a read-only lower layer and a per-coder writable upper layer. Now the file *contents* are read into the OS page cache once and shared across every coder on the box; each coder's edits are just its private upper layer. This is the real per-worker memory reduction on large repos. It requires overlay/btrfs support and must compose with the worker's Landlock/systemd confinement (the writable grant points at the merged view). Ship it as an opt-in tier once the worktree version is solid.

**One concrete detail I can already see.** `commit_and_bundle_delta` hardcodes the working branch name `fed-work`. Git refuses to check out the same branch in two worktrees, so concurrent coders on one shared repo each need a **unique work-branch** (e.g. `fed-work-<coder-id>`). Small parameterization; the delta bundle stays `base_sha..<that branch>`, unchanged.

**The cache.** A per-worker, size-bounded cache of materialized bases keyed by the base commit (or the bundle's content hash). On a hit, skip the download and the clone entirely — just add a worktree. It's a *fleet of local caches*, one per node, not one shared filesystem (each worker has its own disk).

**Disposable behavior.** Deadline the worktree-add / overlay-mount. On timeout, error, or a base that fails a quick sanity check, evict that base from the cache, fall back to a plain full clone for this coder, and emit `degraded(reason)`. Coders' commits add objects to the shared bare repo over time, so prune worktrees after each task and garbage-collect the bare repo periodically.

---

## Slice 2 — Compute the base's expensive derived data once, share it read-only

**The problem.** Some work is identical for every coder on a given base and is genuinely slow: the embeddings used for memory retrieval, and any repo-wide search or symbol index. Each coder redoing it is waste. (Plain file reads are *not* in scope — the OS page cache already makes them fast; caching them buys nothing.)

**The idea.** Compute those base-derived artifacts **once, by the trusted side** — never inside a coder — store them in the shared federation key-value store (JetStream KV) keyed by the base commit, and let confined coders **read** them. Two options for who produces, for the plan to pick: the **dispatcher at enqueue time** (it already has the full repo, so the compute stays off the coder's critical path — the eager choice), or the **first worker** to see a base (lazily, so the first coder pays but the rest don't). Either way, only the trusted producer writes, so untrusted coder code can never poison the cache. Content-addressing (the key includes the base's identity) means invalidation is free: a new base is a new key.

This is the F3 "shared state" slice arriving early, and it retires the repeated-embedding cost already flagged in the memory crate.

**Trust boundary.** Coders get **read-only** access to the cache — they consume, they never write. The lookup deadline is tight; on a miss, a timeout, or the KV being unreachable, the coder just computes locally, exactly as it does today. Never block a coder on the cache.

**Ownership note.** The embedding piece touches the `memory` crate's territory, which is Rahul's (`rahulmranga`). We implement the precompute as a *caller* of memory's embedding, not a modification of the memory crate — or coordinate with Rahul before touching it.

---

## Slice 3 — Fewer, faster model round-trips in the agent loop

This one lives in the core agent loop (`crates/core`, `crates/providers`, `crates/tools`), not the federation layer, and it benefits *every* run, local and remote.

**Connection reuse is already done.** The provider keeps a single pooled HTTP client and reuses it on every turn, so we already pay the TLS/TCP handshake once, not per request. Nothing to build here beyond maybe minor pool/HTTP-2 tuning, which is low priority.

**The real lever is the tool phase.** The loop already accepts several tool calls in one model turn, but it runs them one after another (`for call in resp.tool_calls`). Two changes, both of which only affect timing, never the model's decisions:

- **Encourage batching.** Let the model bundle independent operations into a single turn (don't disable parallel tool calls; nudge it in the coder's system prompt). Fewer turns means fewer round-trips to the model — the part that's actually slow.
- **Run the batch concurrently, safely.** Execute read-only tools (`read`, `search`) at the same time; keep mutating tools (`write`, `edit`, `shell`) in order so nothing races. That's the guard that keeps this side-effect-free: classify each tool as read-only or mutating, parallelize the first, serialize the second.

## How all three report back

Reuse what's there. Each coder's lifecycle event carries a small tag — base *hit* / *miss* / *degraded(reason)* — on the `FanoutEvent` stream, so the orchestrator, the TUI, and `/fleet` can see when a fast path was skipped and why. The `WorkResult` gains a compact per-task perf/outcome field. Quiet misses increment a counter; systemic degradation raises a distinct, loud signal so it's visible rather than a slow silent decline.

## Scope, sequencing, non-goals

Three independent slices, each its own plan, shipped in this order:

1. **Slice 1 (shared base)** first — self-contained on the worker, no new cross-node infrastructure, the biggest reliable memory/setup win. Ship the git-worktree version; overlay/btrfs as a follow-up tier.
2. **Slice 3 (loop latency)** second — independent of federation, broad benefit to all runs, but touches the hot-path loop so it wants care.
3. **Slice 2 (precompute cache)** last — most new infrastructure (JetStream KV) and the only one needing cross-crate coordination (memory/Rahul).

**Non-goals:** a general content-addressed VFS beyond CoW of the base; caching plain file reads; coder-writable caches; egress restriction (that's the separate systemd-sandbox exploration); making the model itself faster.

## Testing

- **Slice 1:** unit-test the shared-bare + per-coder-worktree path (unique branches, delta round-trips unchanged) against the existing `repo.rs` tests; a fail-fast test that a corrupt/slow base falls back to full clone; on dev-cx53, a two-coders-one-base run confirming shared objects + independent deltas.
- **Slice 2:** unit-test content-addressed keys + the read-only trust boundary + local-recompute fallback on KV miss/timeout; the live KV path is a dev-cx53 check.
- **Slice 3:** unit-test the read-only-parallel / mutating-serial classification and that a multi-tool-call turn executes concurrently for reads; confirm output is identical to the serial path.
