# Tools, Radio & Companion

## Built-in tools

All tools are sandboxed to the current working directory (canonicalized). Path traversal (`..`) and symlink escapes are blocked.

### `read_file`

Read file contents within the project root.

```json
{ "path": "src/main.rs" }
// → file content
```

### `write_file`

Create or overwrite a file.

```json
{ "path": "src/new.rs", "content": "fn main() {}" }
// → "ok"
```

### `search`

Regex search across the project. Returns file paths + matching line numbers.

```json
{ "pattern": "fn run_task", "path": "crates/core" }
// → matches with context
```

Max 200 results. Uses `ripgrep` under the hood.

### `run_shell`

Execute a shell command. 120s timeout, 100 KB output cap. Process is killed on drop (no orphans).

```json
{ "command": "cargo build 2>&1" }
// → stdout + stderr
```

## Radio

An in-TUI ambient loop of one bundled track — "Standing-Onde" by 8bit-Wraith
— embedded in the binary at compile time and played through `rodio`. No
network fetch, no external tool, no install step. Runs on a dedicated OS
thread so audio never blocks the UI.

### Commands

| Command | Action |
|---|---|
| `/radio pause` | Pause/resume playback |
| `/radio next` | Restart the track from the beginning |
| `/radio stop` | Stop playback (starts again on `/radio next`) |

## Speak

Reads assistant responses aloud via the OS-native TTS engine (`crates/tts`, AVSpeechSynthesizer/NSSpeechSynthesizer on macOS). No models, no network fetch. Off by default.

### Commands

| Command | Action |
|---|---|
| `/speak` | Toggle voice output on/off |
| `/speak on` / `/speak off` | Explicitly enable/disable |
| `/speak stop` | Interrupt the current utterance |

### Shortcuts

| Key | Action |
|---|---|
| `Ctrl-P` | Toggle pause/resume |
| `Ctrl-N` | Skip to next track |

### Cache

Downloads are cached at `~/.cache/entheai/radio` keyed by YouTube video ID. Repeat plays are instant.

### Status bar

```
♪ Song Name
♪ Song Name (paused)
```

Appears in magenta. Clears on stop or queue empty.

## Companion

A borderless, always-on-top 180×180 window showing a QR code for phone pairing over Tailscale. Launched automatically when the TUI starts.

### QR code

Encodes:
- Session UUID
- Tailscale MagicDNS hostname
- Port (9876, for future comms client)
- Working directory

### Config

```toml
[companion]
enabled = true          # spawn on TUI start
always_on_top = true    # float above other windows
```

### CLI

```bash
# Suppress companion for a session
entheai --no-companion
```
