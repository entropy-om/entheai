---
id: companion-radio
title: "Companion & Radio"
group: "The visual TUI"
order: 2
badgeText: "Companion · Radio"
badgeColor: teal
---

Two small extras that run alongside the terminal session: a desktop companion window and an in-TUI music player.

## Companion

A borderless, always-on-top 180×180 window that spawns automatically when the TUI starts. It shows a QR code for pairing a phone to the session over Tailscale, and doubles as a glanceable status light — it glows and pulses differently depending on whether the agent is idle, working, waiting on a permission prompt, or has hit an error. Click it to copy the session URL to your clipboard; it fades out when the session ends.

The QR code encodes the session ID, Tailscale MagicDNS hostname, port, and working directory.

```toml
[companion]
enabled = true          # spawn on TUI start
always_on_top = true    # float above other windows
```

```bash
# Suppress the companion window for a session
entheai --no-companion
```

## Radio

An in-TUI music player. Give it a YouTube URL and it downloads the audio in the background (via `yt-dlp`) and plays it through your speakers (via `rodio`) — on a dedicated thread, so audio never blocks the UI.

> [!NOTE]
> Requires `yt-dlp` on your `$PATH`: `brew install yt-dlp`.

| Command | Action |
|---|---|
| `/radio <url>` | Download and queue a YouTube track |
| `/radio add <url>` | Same as above |
| `/radio pause` | Pause/resume the current track |
| `/radio next` | Skip to the next track |
| `/radio stop` | Stop and clear the queue |

| Key | Action |
|---|---|
| `Ctrl-P` | Toggle pause/resume |
| `Ctrl-N` | Skip to next track |

Downloads are cached at `~/.cache/entheai/radio`, keyed by video ID — repeat plays are instant. The now-playing status appears in the status bar (`♪ Song Name`) and clears on stop or when the queue empties.
