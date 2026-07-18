# entheai companion — design & implementation

**Status**: implemented · 2026-07-18

## 1. What it is

A 180×180 px borderless always-on-top floating window spawned alongside each
entheai session. It shows a breathing animated glow with a QR code encoding
the session identity, and reacts to session state (idle/working/permission/error)
via Unix socket IPC. Clicking copies the session URL to clipboard. When the
session ends, it fades out and exits.

## 2. Architecture

```
┌─ entheai (main) ──────────────────────┐
│  agent loop / TUI                      │
│  ┌──────────────────────────┐          │
│  │ CompanionHandle           │          │
│  │  - spawns child process   │          │
│  │  - binds UnixListener     │          │
│  │  - tokio task forwards    │          │
│  │    StateChange JSON lines  │          │
│  │  - TUI pushes state events │          │
│  └──────────┬───────────────┘          │
└─────────────┼──────────────────────────┘
              │ $TMPDIR/entheai-<sid>.sock
┌─────────────┼──────────────────────────┐
│  entheai-companion (child)              │
│  winit borderless window                │
│  softbuffer CPU pixel buffer            │
│  state-driven animation                 │
│  click-to-copy (arboard)                │
│  fade-out on disconnect                 │
└─────────────────────────────────────────┘
```

## 3. Crate structure

```
crates/companion/
├── Cargo.toml
└── src/
    ├── lib.rs       # pub mod qr, render, state
    ├── main.rs      # CLI parse, winit window, event loop, socket reader
    ├── qr.rs        # QR code generation (SessionPayload → QrGrid)
    ├── render.rs    # AnimationState + per-frame pixel buffer rendering
    └── state.rs     # State enum + StateChange wire format
```

## 4. Dependencies

```toml
winit = "0.30"         # borderless floating window
softbuffer = "0.4"      # CPU pixel buffer
qrcode = "0.14"         # QR matrix generation
arboard = "3"           # clipboard for click-to-copy
serde / serde_json      # wire format
clap / uuid / anyhow    # CLI + errors
```

No GPU deps — Metal was attempted but blocked by `objc2` v0.6 type system
incompatibilities with raw Metal API calls. For a 180×180 window at 24fps,
CPU rendering via softbuffer costs <0.01% GPU time.

## 5. Unix socket protocol

Single-direction JSON lines. Session writes, companion reads. No handshake.

**Socket**: `$TMPDIR/entheai-<session-id>.sock`

```json
{"state":"idle"}
{"state":"working"}
{"state":"permission_pending","tool":"run_shell","args":"cargo build"}
{"state":"error","message":"provider timeout after 30s"}
```

| Field | Type | When |
|---|---|---|
| `state` | `"idle" \| "working" \| "permission_pending" \| "error"` | Every event |
| `tool` | string, optional | Only `permission_pending` |
| `args` | string, optional | Only `permission_pending` |
| `message` | string, optional | Only `error` |

**Lifecycle**:
- Session binds socket, spawns companion, sends initial `working`
- TUI pushes state transitions (idle ↔ working, permission_pending)
- One-shot sends `working` once, exit closes socket → fade-out
- Companion reads non-blocking in winit `AboutToWait`, parses lines
- EOF or error → starts 500ms fade-out, then `target.exit()`

## 6. Animation states

Smooth 300ms lerp transitions between states. All rendered CPU-side in a single
pixel buffer pass.

| State | Glow color | Pulse | Spinner | QR | Glyph |
|---|---|---|---|---|---|
| `idle` | Teal `#00e5ff` | 3s, 20-60% | None | 100% | None |
| `working` | Teal `#00e5ff` | 1.5s, 30-80% | Orbiting dot | 100% | None |
| `permission_pending` | Magenta `#ff00e5` | 1s, 40-100% | None | 40% | "?" |
| `error` | Red `#ff4444` | 4s, 10-30% | None | 30% | None |
| `fading` | Any → transparent | Decay to 0 | – | Fades | Fades |

**Orbiting spinner**: 3px teal dot at 55% window radius, 1 rev per 2s.

**Permission "?"** : 5×7 pixel bitmap, magenta, alpha-pulsed in sync with glow.

**Fade-out**: per-pixel alpha channel reduced from 255→0 over 500ms. Glow, QR,
spinner, and glyph all fade together. Window exits when alpha reaches 0.

## 7. Click-to-copy

Clicking anywhere on the companion copies `http://<host>.local:9876/session/<sid>`
to clipboard via `arboard`. Visual feedback: glow boosts to 100% for 200ms.
No hover state, no cursor change — discoverable easter egg. Silent on clipboard
failure.

## 8. QR code

Encodes a JSON payload via `qrcode` crate (medium error correction):

```json
{"v":1,"sid":"a1b2c3d4","host":"mac-top.peterlodri-sec.ts.net","port":9876,"cwd":"/Users/peter/workspace/entheai"}
```

Hostname resolves Tailscale MagicDNS first, then `hostname.local` fallback.
The URL is a placeholder until the `comms` crate exists.

## 9. Configuration

```toml
[companion]
enabled = true          # spawn companion (default: true)
always_on_top = true    # float above other windows (default: true)
```

CLI: `--no-companion` disables for the session.

## 10. Main binary integration

`CompanionHandle` in `bin/entheai/src/main.rs`:
- Binds `UnixListener` at `$TMPDIR/entheai-<sid>.sock`
- Spawns tokio task: accept → forward `StateChange` JSON lines from mpsc
- Spawns companion child with `--socket <path>`
- Holds `UnboundedSender<StateChange>` for state pushes
- On drop: kills child, removes socket file

TUI integration: `entheai_tui::run()` accepts `Option<UnboundedSender<StateChange>>`.
Sends `idle`/`working`/`permission_pending` at corresponding state transitions.

## 11. CLI

```
entheai-companion --session-id <UUID> [--host <HOST>] [--port <PORT>]
                  [--cwd <CWD>] [--socket <PATH>] [--no-always-on-top]
```

## 12. What's deferred

- **Metal GPU rendering**: blocked by objc2 v0.6 type system. Needs a `-sys` crate or `objc2::ffi::msg_send!`. Low priority — CPU rendering is fast enough.
- **Remote session resume**: the QR URL is a placeholder. Requires `comms` crate (HTTP server + Tailscale integration).
- **Per-frame surface recreation**: `softbuffer::Surface` is created each frame. Micro-optimization blocked by winit borrow checker. Negligible cost at 180×180 px.
- **Position configuration**: always bottom-right, 20px margin. Trivial to add `--position` later.
