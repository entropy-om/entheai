# entheai companion — design spec

**Status**: draft · 2026-07-18

## 1. What it is

A tiny, animated, always-on-top floating window that spawns alongside each
entheai session. It shows a pulsing "heartbeat" animation and a QR code that
encodes the session identity so the user can remotely resume or drop into the
session from another device (phone, laptop, tablet).

Think of it as a "session beacon" — minimal, beautiful, functional.

```
┌──────────────────────┐
│                      │
│     ◉  pulsing       │
│    ╱ │ ╲  glow       │
│   ▐▄▄▄▄▄▄▌           │
│   █ QR  █            │
│   █ code█            │
│   ▐▄▄▄▄▄▄▌           │
│                      │
│  session active      │
│  mac-top · zen/v4    │
└──────────────────────┘
  ~180×180 px · no chrome
  floating · ⌘ always on top
```

## 2. User story

1. User starts `entheai` (one-shot or TUI). A companion window appears in the
   bottom-right corner.
2. The companion pulses gently — a bioluminescent teal glow breathing in/out —
   indicating the session is alive.
3. User points their phone camera at the QR code → opens a URL that lets them
   resume the session, watch progress, or send a prompt remotely.
4. When the session ends (agent exits, TUI closes, or process dies), the
   companion window fades out and closes.
5. User can disable it per-session (`--no-companion`) or globally in config.

## 3. Design decisions

### 3.1 Separate binary — `entheai-companion`

The companion is a **separate tiny binary**, spawned as a child process by the
main `entheai` binary. Why:

- **Main-thread isolation.** `winit` on macOS requires the event loop on the
  main thread. The main `entheai` process already owns the main thread for
  tokio + the TUI. A child process sidesteps this entirely.
- **Crash isolation.** If the companion crashes, the main session is unaffected.
- **Clean lifecycle.** The child dies when the parent exits (or we kill it).
- **Zero coupling.** Communication is CLI args + a one-way status file or
  Unix socket — no shared memory, no tangled async runtimes.

### 3.2 Native macOS window via `winit` + `softbuffer`

- **`winit`** — creates a borderless, non-resizable, floating window with
  `NSFloatingWindowLevel` (always on top). No title bar, no dock icon
  (`.with_visible(false)` on the NSApplication activation policy — LSUIElement).
- **`softbuffer`** — writes raw RGBA pixel buffers to the window surface.
  Minimal, no GPU pipeline needed for a 180×180 widget.
- **`qrcode`** crate — generates the QR matrix.
- **`image`** crate — renders QR + glow into the pixel buffer each frame.

Alternatives considered:

| Approach | Verdict |
|---|---|
| `wry` (webview) | WebKit dependency is ~50 MB; overkill for 180×180 px |
| `tauri` | Even heavier; full app bundle |
| `objc` + raw AppKit | Powerful but verbose; winit wraps this well enough |
| Terminal overlay (Kitty protocol) | Can't float over other apps; defeats "always on top" |
| `pixels` crate | Good but wraps wgpu; `softbuffer` is simpler for a tiny widget |

### 3.3 QR code content

The QR encodes a JSON payload (kept small for scannability at 180 px):

```json
{
  "v": 1,
  "sid": "a1b2c3d4",
  "host": "mac-top.peterlodri-sec.ts.net",
  "port": 9876,
  "cwd": "/Users/peter/workspace/entheai"
}
```

| Field | Meaning |
|---|---|
| `v` | Schema version (for forward compat) |
| `sid` | Session ID (UUID v4, generated at session start) |
| `host` | Tailscale MagicDNS hostname (or `hostname.local` fallback) |
| `port` | Local HTTP endpoint for session resume (future: `comms` crate) |
| `cwd` | Working directory so a remote client knows the project |

**v0.1 fallback**: If Tailscale is not running, `host` falls back to
`<hostname>.local`. If no HTTP server exists yet (pre-`comms`), the QR still
encodes the session identity — useful for manual SSH resume or copy-paste.

### 3.4 Animation

Simple, low-CPU, no GPU shader:

- **Breathing glow**: a radial gradient behind the QR code that pulses
  between 20% and 60% opacity on a ~3-second sine wave. Teal (`#00e5ff`) on
  the entheai dark background (`#0a0f14`).
- **Spinner indicator**: a subtle rotating arc or dot orbiting the companion
  when the agent is actively working (tool calls in flight). Idle when waiting
  for user input.
- **Frame rate**: 24 fps (timer-based redraw). CPU-only; a 180×180 pixel
  buffer is ~130 KB — trivial to recompute.

### 3.5 States

| State | Visual |
|---|---|
| **Alive (idle)** | Breathing glow, QR visible, "(model)" label |
| **Working** | Breathing glow + orbiting spinner, "thinking…" label |
| **Permission pending** | Glow turns magenta, QR pulses faster, "allow tool?" label |
| **Error** | Glow turns red, QR dims, "error" label |
| **Exiting** | Fade-out animation over 500 ms, then close |

### 3.6 Always-on-top & placement

- Default: **always on top** (`NSFloatingWindowLevel`).
- Configurable: `always_on_top = false` moves it to normal window level
  (sits behind active windows).
- Default position: **bottom-right**, 20 px margin from screen edge.
- Configurable positions: `top-left`, `top-right`, `bottom-left`,
  `bottom-right`.
- Click-through? No — the companion is interactive (hover for details,
  click to copy session URL). But it ignores focus (`.with_focus(false)` on
  macOS — it's a floating panel, not stealing keyboard focus).

## 4. Implementation plan

### 4.1 New crate: `crates/companion/`

```
crates/companion/
├── Cargo.toml
└── src/
    ├── main.rs           # binary entry: parse args, init window, run loop
    ├── window.rs          # winit window + event loop
    ├── render.rs          # pixel buffer: glow + QR + labels
    └── qr.rs              # QR code generation + session payload
```

### 4.2 Dependencies

```toml
[dependencies]
winit = "0.30"              # window creation, event loop
softbuffer = "0.4"           # CPU pixel buffer → window surface
qrcode = "0.14"              # QR code matrix generation
image = "0.25"               # pixel buffer assembly (no features needed)
serde = { workspace = true }
serde_json = { workspace = true }
clap = { workspace = true }  # CLI arg parsing
uuid = { version = "1", features = ["v4"] }
```

Total dependency weight: ~15 crates (winit pulls in a few). All pure Rust, no
C dependencies beyond system frameworks (AppKit, CoreGraphics — already present
on macOS).

### 4.3 CLI interface

```
entheai-companion 0.1.0

USAGE:
    entheai-companion [OPTIONS] --session-id <SESSION_ID>

OPTIONS:
    --session-id <SESSION_ID>    Session UUID (required)
    --host <HOST>                Tailscale or local hostname [default: hostname.local]
    --port <PORT>                Session HTTP port [default: 9876]
    --cwd <CWD>                  Working directory [default: current dir]
    --position <POSITION>        bottom-right, top-right, bottom-left, top-left [default: bottom-right]
    --no-always-on-top           Disable always-on-top
    --opacity <OPACITY>          Window opacity 0.1-1.0 [default: 0.92]
    --model <MODEL>              Model label for display
```

### 4.4 Main binary integration (`bin/entheai/src/main.rs`)

Before starting the agent or TUI:

1. Generate a `session_id` (UUID v4).
2. Resolve the Tailscale hostname (check `tailscale status` or fall back to
   `hostname`). If Tailscale is installed, derive MagicDNS from `tailscale
   status --json`.
3. Spawn `entheai-companion` as a child process:
   ```rust
   let companion = Command::new("entheai-companion")
       .args(&["--session-id", &session_id, "--host", &host, ...])
       .spawn()?;
   ```
4. On exit (normal or error), kill the child:
   ```rust
   let _ = companion.kill();
   ```

The companion is only spawned when:
- Config `[companion].enabled = true` (default)
- CLI `--no-companion` is NOT passed

### 4.5 Configuration (`entheai.toml`)

```toml
[companion]
enabled = true
always_on_top = true
position = "bottom-right"   # top-left | top-right | bottom-left | bottom-right
opacity = 0.92
size = 180                  # pixels (square)
```

## 5. Future: remote session resume (v0.2+)

Once the `comms` and `session` crates exist:

1. The main `entheai` process starts a **local HTTP server** (bound to
   `127.0.0.1` or Tailscale interface) that accepts session-control requests.
2. The QR code encodes a URL like:
   `http://mac-top.peterlodri-sec.ts.net:9876/session/a1b2c3d4`
3. Scanning opens a lightweight web UI:
   - **Watch**: live token stream + tool activity (read-only).
   - **Chat**: send a prompt into the running session.
   - **Resume**: if the session is paused (permission gate), approve/deny.
   - **Kill**: terminate the session remotely.
4. Authentication: short-lived token embedded in the URL (rotated every 60s).
   Or Tailscale ACL restricts access to the tailnet.

## 6. What this is NOT

- **Not a second TUI.** It has no text input, no chat history, no tool output.
- **Not a system tray icon.** It's a window, visible on screen.
- **Not a dock icon.** LSUIElement = no dock presence.
- **Not a notification daemon.** It doesn't pop up alerts.
- **Not a remote desktop.** The QR is a key, not a screen share.

## 7. Open questions

1. **Should the companion be a dock icon or a floating window?**
   → Floating window (current design). A dock icon is too conventional;
   the companion should feel like a living organism next to your work.

2. **Should it work without Tailscale?**
   → Yes. Falls back to `hostname.local`. The QR is still useful — scan it
   to copy the session ID, then SSH in manually.

3. **Multiple sessions = multiple companions?**
   → Yes. Each `entheai` process spawns its own companion. They stack or
   the user positions them. This is fine — the user rarely runs >2 sessions.

4. **Should it have a right-click menu?**
   → Maybe later. "Copy session URL", "Hide companion", "Quit session"
   would be useful. Not in v0.1.

5. **What about the TUI? Should the companion show TUI-specific info?**
   → No. The companion is mode-agnostic — it works for both one-shot and
   TUI sessions. The status labels change ("thinking" / "waiting" / "error")
   but the TUI is its own thing.

## 8. Visual reference

```
╔══════════════════════════════╗
║                              ║
║       ░░░░░░░░░░░░░░         ║  ← breathing radial glow
║     ░░▒▒▒▒▒▒▒▒▒▒▒▒░░         ║     (teal #00e5ff → dark #0a0f14)
║    ░▒██████████████▒░        ║
║   ░▒██  ▄▄▄▄▄▄  ██▒░       ║  ← QR code (white on near-black)
║   ░▒██  █ ▄▄ █  ██▒░       ║      with 2-module quiet zone
║   ░▒██  █ █▀ █  ██▒░       ║
║   ░▒██  █ ▀▄ █  ██▒░       ║
║   ░▒██  ▀▀▀▀▀▀  ██▒░       ║
║    ░▒██████████████▒░        ║
║     ░░▒▒▒▒▒▒▒▒▒▒▒▒░░         ║
║       ░░░░░░░░░░░░░░         ║
║                              ║
║     ◉ zen/deepseek-v4-pro   ║  ← model label (dim, small)
║       mac-top · idle         ║  ← hostname + status
╚══════════════════════════════╝
```

The window has **no chrome** — no title bar, no close/minimize/zoom buttons,
no resize handle. The background is the breathing glow gradient. The QR code
sits centered with a 20 px quiet zone. Below it, the model name and hostname
in small monospace (6-8 pt). The whole window is 180×180 px at 2× retina
(so we render at 360×360 and let the OS scale down).

When working: a small orbiting dot traces a circle around the QR code.
When permission is pending: the glow shifts from teal to magenta, pulses
faster (1.5s cycle instead of 3s), and a "?" appears over the QR.
