# Lead review — core crates (memory · radio · companion)

**Date:** 2026-07-18 · **Reviewer:** lead pass (3 parallel read-only reviews + consolidation) · **For:** the sessions owning these crates.

Scope: the three core Rust crates I did not author. Gate was **red on two counts** when I started (memory unformatted + a `too_many_arguments` clippy error the memory-into-core wiring introduced) — both fixed, so **the workspace gate is green again** (`./scripts/check.sh`: fmt + clippy `-D warnings` + **118 tests**). Two **Critical** bugs and one Important were fixed directly (committed); the rest are documented below for the owning sessions.

## Fixed in this pass (committed)
| Commit | Fix |
|---|---|
| `e20bf7c` | Gate: `#[allow(clippy::too_many_arguments)]` on `run_task_with_memory`; formatted the memory crate |
| `6d5e282` | **memory Critical:** char-safe tool-output preview (`runtime.rs:197` byte-slice panicked on non-ASCII); **memory Important:** task-scoped learning keys; **radio Critical:** reject flag-injection URLs at `download()` |

---

## `entheai-memory`
Two-tier / 5-namespace persistent store (SQLite + flat cosine + OpenAI-compatible embedder) plus the new `MemoryRuntime` (`runtime.rs`) wired into `run_task_with_memory` (pre-task retrieval → large-output spillover → post-task trajectory/learning recording). **Gate: clippy clean, 24 tests pass.** Earlier-flagged issues **all confirmed FIXED**: mutex-poisoning cascade (`store.rs:89` recovers via `unwrap_or_else(into_inner)`), fabricated `created_at`-on-update (`RETURNING`), missing embedder HTTP timeout (30s).

- ✅ **Critical (FIXED) — panic on non-ASCII tool output.** `runtime.rs:197` sliced `&evidence.result[..500]` by raw byte → panics mid-UTF-8-char. Now uses the crate's own `truncate_str`.
- ✅ **Important (FIXED) — learning-key collision.** `runtime.rs:301` keyed by `session_id` only (not `task_id`), so a second task in a session silently overwrote the first's learnings. Now `{session_id}/{task_id}/tool/{idx}`.
- ⛔ **Important (OUTSTANDING) — `ToolEvidence.allowed` is hard-coded `true`.** `dispatch_call` computes the real permission decision but only encodes denial into the *result string*, so `run_task_with_memory` (`core/lib.rs:221`) records every call as `allowed: true` → the `"denied"` learning branch (`runtime.rs:285`) is dead code and denials are mislabeled `"failed"`. **Fix:** have `dispatch_call` return the `allowed` bool (or the `Decision`) and thread it into `ToolEvidence`. *(Touches `crates/core` — coordinate with whoever owns `run_task_with_memory`.)*
- ⛔ **Important (OUTSTANDING) — fully silent failures.** `runtime.rs:6` claims "failures produce log diagnostics," but the crate has no `tracing`/`log` dep and emits nothing; in default (non-strict) mode a down embedder silently stops all persistence with no operator signal. **Fix:** add a logging dep + a diagnostic at each swallowed-error site.
- **Minor:** `MemoryError::Embedding` reused for `serde_json` errors; `retrieve_before` uses `break` (not `continue`) on an over-length line, dropping still-fitting lower-ranked results (`runtime.rs:152`); re-embeds full content on every write and loops `store()` serially while `embed_batch` sits unused; hardcoded 30s timeout; `open`/`open_memory` duplicate DDL; dimension-mismatched embeddings silently skipped forever.
- **Test gaps:** no end-to-end embedded-search test (every store test uses `embedder=None`); `record_*` tests assert "no error" but never `get()` back the written content/keys (would have caught the key collision); no `strict:true` test; no multi-byte regression (would have caught the panic); no multi-task-same-session test.

## `entheai-radio`
In-TUI player: `/radio` commands → `RadioCommand` over `std::sync::mpsc` → a dedicated OS thread owning the `!Send` rodio `Sink`; `Add` spawns a downloader thread that shells to `yt-dlp`. **Gate: clippy clean, 6 tests pass. No `unsafe`, no shell-string injection (argv vector).**

- ✅ **Critical (FIXED) — yt-dlp flag-injection RCE.** `/radio add <url>` did no validation; a `-`-prefixed token (`--exec=…`) reached yt-dlp's argv parser → arbitrary command execution. `download()` now rejects non-`http(s)` URLs (a flag can't start with `http`) and passes `--` before the positional. This is the choke point for both `/radio` forms.
- ⛔ **Important (OUTSTANDING) — no timeout / no kill / unbounded downloader threads.** `download()` uses blocking `.output()` with no `Child` retained, no timeout, no cap; each `/radio add` spawns a fresh thread. A hung `yt-dlp` blocks forever and repeated adds pile up blocked threads + live children. **Fix:** `tokio::process` + `kill_on_drop(true)` + `timeout`, retain the `Child` so Stop/Shutdown can cancel.
- ⛔ **Important (OUTSTANDING) — orphaned downloads on Stop/exit.** `Stop`/`Drop for Radio` don't cancel or reap in-flight downloader threads/children; quitting mid-download leaves detached `yt-dlp` processes writing to the cache dir. **Fix:** track + kill/join download handles.
- **Minor:** `.expect("spawn radio thread")` (`lib.rs:94`) panics the whole TUI on thread-spawn failure instead of degrading; `advance()` drops the just-decoded track (and doesn't retry) when the audio device can't open; fully-buffered subprocess output (benign today); consider a host allow-list / reject private-IP literals (SSRF via generic extractor).
- **Test gaps:** URL-validation / arg-construction untested; `advance`/device-failure; `Next`/`TogglePause` transitions; concurrent `Add`; the TUI `add <url>` branch.

## `entheai-companion`
Standalone beacon window (winit + softbuffer + qrcode) rendering a state-aware "breathing" glow + a pairing QR; a one-way Unix-socket **client** consuming newline-delimited `StateChange` JSON from `bin/entheai` (the listener/accept lives there). **Gate: clippy clean, 7 tests pass. No `unsafe`.** I did **not** modify this crate — its Criticals are perf/robustness in a *separate* process (not agent-crashing), the fixes need event-loop restructuring, and the socket issue spans `bin/entheai` — best owned by the companion session. `image` dep confirmed **removed**.

- ⛔ **Critical (OUTSTANDING) — softbuffer `Context`/`Surface` rebuilt every frame** (`main.rs:153-154`) via `.expect()` → surface churn + a per-frame panic landmine. **Fix:** build once after window creation; `.resize()` on size change; handle failure without `.expect()`.
- ⛔ **Critical (OUTSTANDING) — uncapped CPU.** `ControlFlow::Poll` + unconditional `request_redraw` every `AboutToWait`; the 24fps `frame_interval()` is defined but **never called** → a "beacon" pins a core at ~100%. **Fix:** `WaitUntil(next_frame)` gated on `frame_interval()`.
- ⛔ **Important (OUTSTANDING) — partial-line IPC drop** (`main.rs:191`): the read buffer is declared *inside* the drain loop and discarded on `WouldBlock`, so a `StateChange` split across two reads is silently lost. **Fix:** persist the partial-line buffer across `AboutToWait` ticks.
- ⛔ **Important (OUTSTANDING) — QR vanishes** (`render.rs:177`): `module_px = qr_px / qr.size` with no `.max(1)` → a long payload (FQDN + deep cwd) yields `module_px==0` and a blank QR. **Fix:** clamp to ≥1 and/or bound payload growth.
- ⛔ **Important (OUTSTANDING) — Unix-socket trust boundary** (spans `bin/entheai/src/main.rs:126-144`): listener bound in world-writable `temp_dir` at a predictable path with no `chmod`/peer-cred check; the first local process to connect steals the single accept slot and receives the `StateChange` stream (which can carry `tool`/`args`/`message`). **Fix:** bind at `0700`/random path, or authenticate with a shared secret before trusting traffic.
- **Minor:** `uuid` should be a dev-dep (only used in a test); silent connect-failure; `arboard::Clipboard::new()` per click; deprecated winit closure API (`#[allow(deprecated)]`).
- **Test gaps:** IPC path entirely untested; `state.rs` has no serde round-trip test (the exact cross-process contract); no `module_px==0` regression; no click-to-copy test.

---

## Action items (for the owning sessions)
**memory:** thread the real `allowed` decision from `dispatch_call` into `ToolEvidence`; add a logging dep + diagnostics at swallowed-error sites; add an end-to-end embedded-search test + a multi-task-same-session test.
**radio:** move to `tokio::process` with `kill_on_drop` + `timeout` + retained `Child`; cancel/reap downloads on Stop/Shutdown; return a `Result` from `Radio::spawn` instead of `.expect()`; add a URL-rejection unit test.
**companion:** hoist `Context`/`Surface` out of the frame loop + drop the `.expect()`s; apply the `frame_interval()` budget with `WaitUntil`; persist the partial-line IPC buffer; clamp `module_px ≥ 1`; harden the socket (perms/peer-check, coordinated with `bin/entheai`).
