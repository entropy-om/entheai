# TUI Guide

`entheai`'s terminal UI is built with [ratatui](https://ratatui.rs). It behaves as an interactive agentic development environment — type messages, stream LLM thinking & tool execution inline, view live fan-out swarms, monitor the brain panel, and enjoy procedural ambient audio.

---

## Layout

```
┌────────────────────────────────────────────────────────────────────────┐
│ entheai · zen/deepseek-v4-pro · idle · wk 0 · nats ● · ctx 12% ♪ Mesa   │ ← status bar
├────────────────────────────────────────────────────────────────────────┤
│ you> explain the router crate                                          │
│ entheai> The router crate selects the best model per role...           │ ← scrollable history
│   tool> ⚙ read_file(crates/router/src/lib.rs)                         │
│   tool>   ↳ pub struct RouterConfig { ... }                            │
│ ⠋ running read_file                                            2.3s    │ ← live progress
├────────────────────────────────────────────────────────────────────────┤
│ 🧠 BRAIN PANEL: Faculties | Fleet Swarm | 5-NS Memory                  │ ← brain panel
├────────────────────────────────────────────────────────────────────────┤
│ ┌ message ───────────────────────────────────────────────────────────┐ │
│ │ explain the router crate                                           │ │ ← input box
│ └────────────────────────────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────────────────┘
```

---

## Key Bindings

| Key | Context | Action |
|---|---|---|
| `Enter` | Input non-empty | Submit prompt |
| `Esc` (double) | Working | Interrupt/stop running task |
| `Esc` / `q` | Idle, empty input | Quit TUI |
| `Ctrl-C` (double) | Any | Force quit |
| `y` / `Y` | Permission modal | Allow gated tool call |
| `n` / `N` | Permission modal | Deny gated tool call |
| `Ctrl-P` | Any | Toggle radio pause/resume |
| `Ctrl-N` | Any | Skip to next procedural radio track |
| `PageUp` / `Up` | Any | Scroll conversation history up |
| `PageDown` / `Down` | Any | Scroll conversation history down |

---

## Slash Commands

| Command | Action |
|---|---|
| `/radio procedural` | Activate infinite procedural ambient radio (seeded by `~/Downloads/Mesa*`) |
| `/radio seed [pattern]` | Seed procedural audio generation from a custom path/glob |
| `/radio <url_or_path>` | Play/queue a YouTube video or local audio file (`.mp3`, `.m4a`, `.wav`, `.flac`) |
| `/radio pause` / `/radio next` / `/radio stop` | Control radio playback |
| `/clear` | Clear message history (preserves system prompt) |
| `/workers` | View active federation worker nodes and task assignments |
| `/fanout [prompt]` | Trigger fan-out execution into isolated git worktrees |

---

## Permission Flow

When the agent executes a gated tool (outside the allowlist or non-YOLO mode), an interactive modal appears:

```
┌──────────────────────────────────────────────────┐
│ allow run_shell(git status)?   [y]es / [n]o     │
└──────────────────────────────────────────────────┘
```

* Press **`y`** to approve or **`n`** to deny.
* Use `--yolo` flag (`entheai --yolo`) to auto-approve all tool calls for unattended workflows.

---

## Swarm Graph & Brain Panel

During `--fanout` runs, a live ASCII swarm graph appears above the input box, rendering sub-agent dependencies, current worktree status, model assignments, and completion states in real-time.

The **Brain Panel** displays:
* Active cognitive faculties & active router tier
* NATS federation connectivity (`nats ●` / `nats ○`)
* 5-namespace memory context ratio (`ctx %`)
