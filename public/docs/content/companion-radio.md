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

An in-TUI ambient loop of one bundled track — "Standing-Onde" by 8bit-Wraith — embedded in the binary at compile time and played through your speakers (via `rodio`) on a dedicated thread, so audio never blocks the UI. No network fetch, no external tool, nothing to install.

| Command | Action |
|---|---|
| `/radio pause` | Pause/resume playback |
| `/radio next` | Restart the track from the beginning |
| `/radio stop` | Stop playback (starts again on `/radio next`) |

| Key | Action |
|---|---|
| `Ctrl-P` | Toggle pause/resume |
| `Ctrl-N` | Restart the track |

The now-playing status appears in the status bar (`♪ Standing-Onde — 8bit-Wraith`) and clears on stop.
