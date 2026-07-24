---
id: companion-radio
title: "Companion, Radio & Speak"
group: "The visual TUI"
order: 2
badgeText: "Companion · Radio · Speak"
badgeColor: teal
---

Peripheral integrations that run concurrently with the terminal session:

## 1. Companion Window (`crates/companion`)

A 180×180 px borderless, always-on-top floating window (winit + softbuffer) spawned when `[companion].enabled = true`:

- **QR Code Pairing**: Encodes session ID, Tailscale MagicDNS hostname, port, and working directory for phone/device pairing.
- **State Telemetry**: Listens on a Unix socket (`$TMPDIR/entheai-<sid>.sock`) for `StateChange` events:
  - *Idle*: Slow teal pulse (3s cycle)
  - *Working*: Fast teal pulse (1.5s) + orbiting spinner
  - *Permission Pending*: Magenta pulse (1s) + "?" glyph
  - *Error*: Red dim pulse (4s)
- **Click Action**: Click window to copy `http://<host>.local:9876/session/<sid>` to system clipboard.
- **CLI Flag**: `entheai --no-companion` disables the window for a session.

## 2. Audio Radio (`crates/radio`)

Zero-fetch ambient audio generator running through `rodio` on a dedicated thread:

- **Station 1 ("Standing-Onde")**: Bundled track by 8bit-Wraith embedded directly in the binary (`include_bytes!`).
- **Station 2 ("Mirror in F — Fable's seed")**: Infinite, deterministic Arvo Pärt tintinnabuli generator synthesized sample-by-sample in pure `std` math, seeded by `b"FABLE"`.
- **Controls**: `/radio pause` (`Ctrl-P`), `/radio next` (`Ctrl-N` — cycles stations), `/radio stop`.

## 3. OS-Native Speech Output (`crates/tts`)

Assistant responses read aloud via native OS TTS synthesizer (`AVSpeechSynthesizer` on macOS):

- Toggle via `/speak`, `/speak on`, `/speak off`, or interrupt with `/speak stop`.
- Operates entirely offline with zero network calls or heavy models.
