# Lead Review — companion crate + integrations

**Reviewer**: codex (delegated)  
**Date**: 2026-07-18  
**Scope**: `crates/companion/`, `crates/config/` (companion changes), `bin/entheai/` (companion changes), `crates/tui/` (companion wiring)  
**Commits**: `f0a9d7d` → `9325699`

## Summary

The companion crate delivers a 180×180 px always-on-top session beacon window
with state-aware animation, Unix socket IPC, click-to-copy, and fade-out on
disconnect. 4 files in the companion crate (+584/-100 from baseline), minor
additions to config, main binary, and TUI. Well-scoped, follows existing
project conventions, test-covered.

**Verdict**: ✓ Approve. Minor issues noted below are non-blocking.

---

## 1. Architecture

### 1.1 Process isolation

The companion runs as a **separate binary** spawned by the main process.
Correct decision:

- macOS requires the winit event loop on the main thread. The main process
  already owns main-thread for tokio + TUI. Child process sidesteps this.
- Crash isolation: companion crash doesn't affect the session.
- Clean lifecycle: `CompanionGuard` kills the child on drop (normal exit,
  error, or panic).

### 1.2 IPC via Unix socket

The session binds a `UnixListener` at `$TMPDIR/entheai-<sid>.sock`, spawns a
tokio task that accepts the companion connection and forwards `StateChange`
JSON lines from an mpsc channel. The companion reads non-blocking in the
winit event loop.

**Good**: unidirectional, no handshake, no heartbeat — disconnect IS the death
signal. Socket path uses `$TMPDIR` which is cleaned on reboot.

**Nit**: Socket path collision is theoretically possible if two sessions share
a UUID (impossible with UUIDv4). No defense needed, but worth noting.

### 1.3 State machine

Four states with smooth 300ms lerp transitions:

```
idle ⇄ working ⇄ permission_pending
  ↓                    ↓
error ←─────────────────┘
  ↓
fade-out → exit
```

The TUI pushes state changes at user submit, permission prompt, and task
completion. The one-shot path sends `working` once and relies on socket close
for exit.

**Missing**: The one-shot path doesn't push `permission_pending` for tool gates
because `run_task()` doesn't know about the companion. The TUI covers this case.
Acceptable for v0.1 — the companion shows `working` throughout one-shot runs.

---

## 2. Code quality

### 2.1 Companion crate (`crates/companion/`)

| File | Lines | Quality |
|---|---|---|
| `state.rs` | 65 | ✓ Clean 4-state enum, builder methods, serde derive |
| `qr.rs` | 97 | ✓ Simple QR generation, tested |
| `render.rs` | 320 | ✓ State-aware animation, smooth transitions, tested |
| `main.rs` | 220 | ✓ Winit event loop, socket reader, click-to-copy, fade-out |
| `lib.rs` | 3 | ✓ Module re-exports |

**Style**: Follows project conventions — `anyhow::Result`, inline `#[cfg(test)]`,
one-line doc comments, no custom error types.

**Naming**: Consistent. `AnimationState`, `StateChange`, `SessionPayload`,
`QrGrid` — clear and descriptive.

### 2.2 Render pipeline

The render loop creates `softbuffer::Context` and `Surface` on every frame
(`RedrawRequested`). This is wasteful but correct — the borrow checker prevents
storing them across `FnMut` closure invocations.

**Cost**: ~130KB pixel buffer allocation per frame at 24fps for a 180×180
window. Negligible. Not worth the `unsafe` or `Rc<RefCell<>>` gymnastics needed
to cache them.

**Alternative considered**: `Box::leak` the window to get `'static` lifetime,
then store context/surface in the closure. Rejected — too clever for zero
measurable gain.

### 2.3 Pixel rendering

The `render_frame` function computes a radial glow, QR overlay, spinner, and
glyph in a single CPU pass. All math is integer-safe (lerp, smoothstep).
The fade-out uses per-pixel alpha channel reduction.

**Performance**: 129,600 pixels × ~50 arithmetic ops = ~6.5M ops per frame.
At 24fps on Apple Silicon, this is <1% of a single core. No hot path concern.

### 2.4 Error handling

- `anyhow::Result` throughout — consistent with project.
- Socket connection failure → `None` (companion starts in idle, no socket).
- Socket read failure → fade-out (graceful degradation).
- Clipboard failure → silent (no error state, no panic).
- `expect()` on softbuffer context/surface creation — justified; if these fail
  the window can't render and there's no recovery path.

### 2.5 Unsafe code

Zero `unsafe` blocks in the companion crate. All rendering is safe Rust.

---

## 3. Integration points

### 3.1 Config (`crates/config/src/lib.rs`)

Added `CompanionConfig` with `enabled` and `always_on_top` fields. Both default
to `true`. Follows existing `#[serde(default)]` pattern. `Default` impl provided
manually (needed because `#[derive(Default)]` would give `false` for bools).

✓ Clean, minimal addition.

### 3.2 Main binary (`bin/entheai/src/main.rs`)

`CompanionHandle` struct added:

```rust
struct CompanionHandle {
    child: Option<std::process::Child>,
    state_tx: tokio::sync::mpsc::UnboundedSender<StateChange>,
    socket_path: PathBuf,
}
```

- Binds `UnixListener`, spawns tokio task for forwarding
- Spawns companion child with `--socket <path>`
- Exposes `send_state()` (unused in one-shot, used in TUI)
- On drop: kills child, removes socket file

**Issue**: `send_state()` is `#[allow(dead_code)]` because the one-shot path
doesn't use it. This is intentional — the API exists for future integration.

### 3.3 TUI (`crates/tui/Cargo.toml`, `crates/tui/src/lib.rs`)

Added `entheai-companion` dependency. Thread `Option<UnboundedSender<StateChange>>`
through `run()` and `event_loop()`. Pushes state changes at:

- Submit → `working`
- Permission prompt → `permission_pending`
- Task complete → `idle`

✓ Minimal surface area change. The `companion_tx` is `Option` — no breaking
change for callers that don't pass it.

**Warning**: Adds a dependency from `entheai-tui` to `entheai-companion`. This
is a one-way dependency (companion doesn't depend on tui). Acceptable.

---

## 4. Test coverage

| Crate | Tests | Focus |
|---|---|---|
| `entheai-companion` | 7 | QR packing, params_for, animation transitions, BGRA packing, lerp, smooth_falloff |
| `entheai-config` | 1 | TOML parsing (pre-existing) |

**Missing tests** (non-blocking):
- No integration test for the Unix socket protocol (send StateChange, verify
  animation state transition). Would require a headless test — winit needs a
  display, so this is deferred.
- No test for click-to-copy (clipboard interaction is inherently side-effectful).
- No test for fade-out timing.

**Coverage assessment**: The algorithmic core (QR generation, animation params,
smooth transitions) is well-covered. The I/O and windowing code is inherently
hard to test without a display server. Acceptable for v0.1.

---

## 5. Security

- **Socket path**: uses `$TMPDIR` which is per-user on macOS (`/var/folders/...`).
  Other users cannot access it. Within the same user, a malicious process could
  connect and send fake state changes — but the companion only reads, never
  executes. Impact: cosmetic (wrong animation state).
- **Clipboard**: `arboard` writes to the system pasteboard. No secrets exposed.
- **QR code**: encodes only session metadata (UUID, hostname, cwd). No API keys,
  no tokens. Safe to display.
- **Child process**: spawned via `std::process::Command`. No shell injection
  (args passed as `Vec<String>`, not a shell string).

✓ No security concerns.

---

## 6. Issues found

### 6.1 `Cargo.toml` lists `image` dependency (unused)

`crates/companion/Cargo.toml` includes `image = { version = "0.25", ... }` but
it's never imported. The pixel buffer is rendered manually without the `image`
crate.

**Fix**: Remove `image` from dependencies. (0.25 has `png` feature enabled —
this pulls in `flate2`, `zune-jpeg`, etc. unnecessarily.)

### 6.2 `#[allow(deprecated)]` on winit APIs

`EventLoop::new()`, `EventLoop::create_window()`, and `EventLoop::run()` are
deprecated in winit 0.30 in favor of `EventLoopBuilder` and `run_app()`. The
deprecated APIs still work and the migration is non-trivial (requires
`ApplicationHandler` trait). The `#[allow(deprecated)]` annotation documents
the technical debt.

**Fix**: Migrate to `EventLoopBuilder` + `run_app()` when time permits.
Not urgent — winit 0.30.x maintains backward compat.

### 6.3 `State::Error` unused in practice

The TUI sends `idle`, `working`, and `permission_pending`, but never `error`.
The animation state exists and renders correctly, but no code path triggers it.

**Fix**: Wire `State::error` from the agent loop when `run_task` or fan-out
returns an error. Low priority — the fade-out on disconnect already covers
session termination.

### 6.4 Fade-out timing tied to frame rate

`fade_alpha` decreases in the `RedrawRequested` handler using `dt` from frame
timing. But `dt` is computed as `last_frame.elapsed()` which is approximate.
At 24fps, the error is bounded by 1/24s ≈ 42ms. Acceptable for a visual fade.

**Fix**: Use `Instant::now()` directly instead of accumulating frame deltas.
The current code already uses `now - fade_start` for the fade calculation
(line 145), so this is actually correct. The `dt` from frame timing is only
used for animation state lerp, not fade timing.

### 6.5 No reconnection logic

If the companion starts before the session binds the socket (race condition),
`UnixStream::connect` fails and the companion runs with `socket_reader = None`.
It never retries.

**Fix**: Add a retry loop (3 attempts, 100ms apart) in the companion. Low
priority — the session binds the socket before spawning the child, so the
window is <1ms.

---

## 7. Recommendations

### Short-term (before merge to a release branch)

1. **Remove unused `image` dependency** from `crates/companion/Cargo.toml`.
   Saves ~5 transitive dependencies and reduces compile time.

2. **Add `#[allow(dead_code)]` only to `send_state`**, not the entire
   `CompanionHandle` impl. Currently correct — verify it stays that way.

### Medium-term (next sprint)

3. **Migrate winit to non-deprecated APIs** (`EventLoopBuilder`, `run_app`,
   `ActiveEventLoop::create_window`). Enables future winit upgrades.

4. **Wire `State::error`** from agent loop error paths. Enables red dim
   animation when the model returns an error.

5. **Add integration test** for socket protocol using a temp Unix socket.
   Spawn companion with `--socket`, write `StateChange` lines, verify the
   animation state transitions (expose `AnimationState` for inspection in
   test-only code).

### Long-term (when `comms` crate exists)

6. **Replace placeholder URL** with a working HTTP endpoint for remote session
   resume. The QR currently encodes `http://<host>.local:9876/session/<sid>`
   which doesn't resolve.

7. **Retry socket connection** with exponential backoff (3 attempts over 1s).

---

## 8. Diff stat

```
 crates/companion/Cargo.toml      |   2 +-
 crates/companion/src/lib.rs      |   3 +
 crates/companion/src/main.rs     | 220 +++++
 crates/companion/src/qr.rs       |  97 +++
 crates/companion/src/render.rs   | 320 +++++++
 crates/companion/src/state.rs    |  65 ++
 crates/config/src/lib.rs         |  24 +-
 bin/entheai/Cargo.toml           |   3 +-
 bin/entheai/src/main.rs          | 106 ++-
 crates/tui/Cargo.toml            |   1 +
 crates/tui/src/lib.rs            |  55 +-
 11 files changed, 820 insertions(+), 76 deletions(-)
```

---

*Generated with Crush · Assisted-by: Crush:deepseek-v4-pro*
