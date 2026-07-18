# TUI Guide

entheai's terminal UI is built with [ratatui](https://ratatui.rs). It behaves like a chat interface — type messages, the agent responds, tools run inline.

## Layout

```
┌──────────────────────────────────────────────────┐
│ entheai · zen/deepseek-v4-pro · idle  ♪ Song    │ ← status bar
├──────────────────────────────────────────────────┤
│ you> explain the router crate                    │
│ entheai> The router crate selects the best...    │ ← scrollable history
│   tool> ⚙ read_file(crates/router/src/lib.rs)    │
│   tool>   ↳ pub struct RouterConfig { ... }      │
│ ⠋ running read_file                        2.3s  │ ← live progress
├──────────────────────────────────────────────────┤
│ ┌ message ────────────────────────────────────┐  │
│ │ explain the router crate                    │  │ ← input box
│ └──────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────┘
```

## Key bindings

| Key | Context | Action |
|---|---|---|
| `Enter` | Idle, input non-empty | Submit prompt |
| `Esc` | Idle | Quit |
| `q` | Idle, input empty | Quit |
| `Ctrl-C` | Idle | Quit |
| `y` / `Y` | Permission modal | Allow tool call |
| `n` / `N` / `Esc` | Permission modal | Deny tool call |
| `Ctrl-P` | Any | Toggle radio pause/resume |
| `Ctrl-N` | Any | Skip to next radio track |
| `PageUp` / `Up` | Any | Scroll history up |
| `PageDown` / `Down` | Any | Scroll history down |
| `Backspace` | Idle | Delete last input character |

## Permission flow

When the agent wants to run a gated tool (not on the allowlist, not in YOLO mode), a modal appears:

```
┌─────────────────────────────────────────────┐
│ allow run_shell(git diff --stat)?  [y]es / [n]o │
└─────────────────────────────────────────────┘
```

Press `y` to allow, `n` to deny. The agent continues with the result (or an "error: permission denied" message fed back to the model).

## YOLO mode

Skip all permission prompts:

```bash
entheai --yolo "fix all clippy warnings"
```

The agent auto-approves every tool call. Use with worktree isolation + test gates as safety net.

## Status bar

The top line shows:

```
entheai · <model> · <state> · ♪ <track>
```

- **model**: Active provider/model (`zen/deepseek-v4-pro`)
- **state**: `idle`, `working…`, `awaiting permission`
- **track**: Currently playing radio track (magenta), if any

## Live progress

While the agent runs, a spinner line shows:

```
⠋ running read_file    2.3s
```

- Spinner animates at ~11 fps (braille frames)
- Tool name + elapsed time shown
- Tool results stream inline under `↳`

## Radio

Built-in music player. See [Radio](radio.md) for details.

```bash
/radio https://www.youtube.com/watch?v=...
/radio pause
/radio next
/radio stop
```

`Ctrl-P` (pause) and `Ctrl-N` (next) work mid-run.
