# Whole-Codebase Review — 2026-07-20

Read-only audit of the entire entheai workspace via 6 parallel reviewers (agent-engine, orchestration, memory, UI, tools/infra, obsidian+bins). Findings are high-confidence real defects only — style/clippy/doc nits excluded (handled separately; dead deps already removed in `f8ec724`). Dominant theme: **unguarded external-input surfaces** — missing timeouts and unbounded buffers on provider/MCP/shell/stream paths.

Legend: **[FIX]** = will fix now · **[RAHUL]** = `crates/memory` is @rahulmranga's, handed off · **[FLAG]** = intentional, surfaced for a decision, not changed.

## IMPORTANT

1. **[FIX] providers — OOM from provider-controlled tool-call `index`.** `crates/providers/src/lib.rs` ~352: `while tcs.len() <= idx { tcs.push(...) }` where `idx = tc["index"].as_u64()`. A stream emitting `{"index": 1000000000}` allocates ~1e9 tuples → OOM. Fix: key by `HashMap<usize,_>` or clamp `idx` to `tcs.len()+K`.
2. **[FIX] providers — no default request timeout.** `OpenAiCompatProvider::new` (~163) uses `reqwest::Client::new()` (no timeout); a provider that accepts TCP but never responds hangs the agent forever (the timeout only exists via `with_inference`, not guaranteed). Fix: default `.timeout(...)` in `new()`.
3. **[FIX] orchestrator — cleanup force-deletes conflicted/failed coder branches (data loss).** `lib.rs` ~643 → `worktree.rs` ~129 `git branch -D` on **every** coder branch, including conflicted/verify-failed ones whose commits are reachable only via that branch; the report then points the user at a branch that was already deleted. Fix: skip `git branch -D` for branches not in `integration.merged`.
4. **[FIX] orchestrator — timed-out/killed coders committed + integrated as "✓".** `lib.rs` ~549 builds a synthetic `CoderRun` for a `None` join; commit (`commit_all -A`) + default `verify=Skipped` → `integrated=true`, landing partial/broken work while its own output says "coder timed out". Fix: skip commit/integration for `TimedOut`/`Killed` runs.
5. **[FIX] orchestrator — worktrees + branches leak on error paths.** The only cleanup is step-6 (`lib.rs` ~643); `?` early-returns after `wt_pool.create` / `integrate(...).await?` leave `entheai-wt-*` dirs + `entheai/<session>/coder-*` branches behind, accumulating across runs. Fix: cleanup via a scope-guard that runs on every exit.
6. **[FIX] mcp — no request/spawn timeout (config exists but unwired).** `crates/mcp/src/lib.rs` ~112 `request()` awaits unbounded; `spawn` never uses `mcp_defaults.spawn_timeout_secs` (declared in config). A server that connects but never returns `initialize` (e.g. `command="cat"`, which echoes the request → parses with an id but no result/error → `continue`) hangs startup forever. Fix: wrap connect/request in `tokio::time::timeout` + thread the config value.
7. **[FIX] mcp — no line-length cap on server output.** `lib.rs` ~60 `next_line()` grows an unbounded `String`; a server streaming bytes with no newline → OOM. Fix: bounded read / error past N bytes.
8. **[FIX] tools/shell — `output_cap` does not bound memory.** `shell.rs` ~59 `.output()` buffers all stdout+stderr in RAM; the cap only truncates *after* collecting. `run_shell({"command":"yes"})` OOMs over the 120s window. Fix: streaming reader that stops at `output_cap` bytes, then kills the child.
9. **[FIX] obsidian — blocking FS I/O on a Tokio worker.** `lib.rs` ~83/98: the sync `apply()` (full `walkdir`+`read_to_string`+temp writes) runs directly on the runtime each debounced tick. Fix: `tokio::task::spawn_blocking`.
10. **[FIX] tui — a hung/panicking run strands the UI in `Working`, unquittable.** `lib.rs` ~488/525/762: run task spawned detached; only `result_rx` resets to `Idle`, and Ctrl-C/Esc/`q` are gated on `idle` while raw mode swallows Ctrl-C. A panic (send never runs) or hang → spinner forever, only SIGKILL escapes. Fix: keep the `JoinHandle`; Ctrl-C while `Working` → `.abort()` + reset to `Idle`.
11. **[FIX] companion — animation `dt` measured against the wrong clock.** `main.rs` ~325 resets `last_frame` in `AboutToWait` right before `request_redraw`; `RedrawRequested` then computes `dt = last_frame.elapsed()` ≈ 0, so `tick(dt)` barely lerps → state-color transitions (teal/magenta/red) smear over seconds instead of 0.3s. Fix: a frame-delta clock separate from the redraw scheduler.

## MINOR

- **[FIX] core** — injected memory context inserted at `len-1` instead of the matched user-message index (`lib.rs` ~164); misplaced when the message list ends with an assistant/tool message.
- **[FIX] orchestrator** — `WorkerPool` entries never reaped (`pool.rs`) → unbounded growth + stale `/workers` list; worktree-pool temp dir (`entheai-wt-<session>`) never removed.
- **[FIX] obsidian** — fence state machine toggles on ``` and `~~~` interchangeably (`generators.rs` ~170); mixed markers desync fence detection (generated-note fidelity only).
- **[FIX] companion** — QR centering `(width - total_qr_px)/2` can underflow → debug panic on a degenerate frame (`render.rs` ~185); use `saturating_sub`.
- **[FIX] tui** (perf) — `LineCache` invalidates on every streamed token → re-wraps the entire scrollback each token (O(n²)/turn). Cache per-message wrapped lines.
- **[FIX] tools/fs** — `read_file`/`write_file` have no size cap (inconsistent with the shell cap; low risk, workspace-confined).
- **[FLAG] radio** — latent stdio-pipe deadlock (`lib.rs` ~321): child pipes drained only after `wait_timeout`; safe today only because output is tiny (`--quiet` + 2 `--print`).

## Handed to @rahulmranga (`crates/memory` — not edited here)

- **IMPORTANT — tri-store drift.** `store.rs` ~196: on upsert, `entries.embedding` is overwritten unconditionally but the `vec_entries` delete lives inside `if let Some(emb)`. Re-storing a key with **no** embedding NULLs `entries.embedding` while leaving the stale vector row; KNN later surfaces a vector that no longer matches the content, and nothing reconciles it (backfill only inserts). Fix: move the `DELETE FROM vec_entries` out of the `if let Some(emb)` guard.
- MINOR — `load_entries` IN-clause can exceed `SQLITE_MAX_VARIABLE_NUMBER` for a public `search(limit)` above ~5.4k (chunk or clamp).
- MINOR — `embed_batch` (`embed.rs` ~67) assumes response order == request order and count parity; sort by `data[i].index` + assert length (latent — store write path uses single `embed`).
- Observation — a single `Mutex<Connection>` serializes all reads+writes, so WAL's concurrent-reader benefit (per the `SqliteStore` doc) is never realized; throughput ceiling, not a bug.

## Intentional defaults surfaced (not changed)

- **[FLAG] config — `PermissionConfig.fanout_auto_approve = true`.** Fan-out sub-agents auto-approve every tool call (incl. `run_shell`) with no prompt. Intentional (coders run in isolated worktrees) but a permissive security default.
- **[FLAG] bin — baked-in production Sentry DSN.** `init_telemetry` falls back to a hardcoded DSN when none is configured, so crash telemetry egresses to a third-party endpoint by default (`send_default_pii: false`; opt-out requires editing config).

## Cleared (checked, no issue)

viz · router · permission (fail-closed) · skills · launcher · entheai-worker · entheai-launch · mapper (path-traversal fix sound) · obsidian VaultWriter confinement (re-confirmed airtight) · memory transactional write atomicity, FTS5-injection defense, RRF/recency math, mutex-poison recovery.
