# Radio Reliability, Usability, and Progress Design

**Date:** 2026-07-19  
**Status:** Approved for implementation planning

## Goal

Make the existing in-TUI radio dependable and easy to understand from URL submission through download, queueing, playback, and shutdown. Preserve the current commands and keyboard shortcuts while adding clear, compact progress and actionable failures.

## Scope

This work covers the `entheai-radio` lifecycle and its TUI integration:

- reliable command delivery and player-thread failure reporting;
- explicit download, queue, playback, pause, cancellation, and failure states;
- visible progress without flooding chat history;
- cancellation of outstanding downloads on stop and shutdown;
- deterministic tests that do not require YouTube or an audio device;
- documentation updates for any user-visible behavior.

It does not add streaming playback, playlists, a queue editor, search, volume controls, or new media providers.

## User Experience

Submitting `/radio <http-or-https-url>` immediately places the track in a fetching state. The TUI status area shows a concise summary such as `radio: fetching`, `radio: playing <title>`, or `radio: paused <title>`. Transient download progress updates replace the current status rather than adding a chat message for every update.

History records only meaningful transitions: the request was accepted, the track was queued or began playing, playback was stopped, or an error requires attention. Errors explain the next useful action, including installing `yt-dlp`, correcting an invalid URL, checking the audio device, or retrying after a timeout.

Existing controls remain stable:

- `/radio <url>` and `/radio add <url>` fetch and queue a track;
- `/radio pause` and `Ctrl-P` toggle pause/resume;
- `/radio next` and `Ctrl-N` skip the current track;
- `/radio stop` stops playback, clears the queue, and cancels outstanding downloads.

Controls that cannot take effect provide feedback instead of silently doing nothing. Examples include pause with no active track and next with an empty queue.

## Architecture

### Radio boundary

`Radio` remains a handle to one dedicated player thread. Sending a command returns a result so the TUI can report a terminated or unavailable player instead of discarding the channel error. Player startup remains lazy with respect to the audio device, preserving headless test behavior.

### Downloader boundary

External process execution moves behind a small downloader interface. Production uses `yt-dlp`; tests use a deterministic fake. Each download receives a stable request ID and a cancellation signal owned by the radio lifecycle.

The production downloader emits structured progress derived from `yt-dlp` output at a deliberately throttled cadence. Progress parsing is best-effort: unfamiliar output must never turn an otherwise successful download into a failure.

### State ownership

The player thread is the single authority for pending downloads, queue order, the current track, and playback state. Download workers send request-scoped results and progress back to that thread. Results for cancelled or superseded requests are ignored and their temporary artifacts are not queued.

Queue ordering follows submission order, not download completion order. A slow first request must not allow later requests to unexpectedly jump ahead. Failed or cancelled requests are removed so the next successful request can advance.

### Events

Events carry enough data for the TUI to update one coherent radio view:

- request accepted/fetching;
- throttled download progress when available;
- queued with title and queue position;
- now playing;
- paused/resumed;
- stopped or queue empty;
- request-scoped cancellation or error;
- player unavailable.

The TUI folds these events into a dedicated radio view model rather than encoding state by modifying display strings such as appending `" (paused)"`.

## Cancellation and Shutdown

`Stop` invalidates all pending request IDs, signals every active downloader, stops the sink, clears the ready queue, and emits a final stopped state. A late worker result cannot restart playback.

Dropping `Radio` sends shutdown and joins the player thread when practical. Shutdown signals active downloaders and ensures child `yt-dlp` processes are killed and reaped. Cancellation must not wait for the five-minute download timeout.

The implementation will bound concurrent downloader work to prevent repeated commands from creating an unlimited number of OS threads or child processes. Pending requests remain ordered and visible while waiting for a worker slot.

## Error Handling

- Validate HTTP(S) URLs before spawning a worker.
- Detect an unavailable `yt-dlp` executable and provide the Homebrew installation command.
- Retain the hard download timeout and kill/reap behavior.
- Drain subprocess output while it runs so verbose progress cannot fill an OS pipe and deadlock the child.
- Treat malformed progress as non-fatal and malformed final output as a clear download failure.
- Continue to the next queued item after a decode, file, download, or audio-device failure.
- Surface player-channel and thread-start failures to the TUI without panicking.

## Testing

Unit tests cover URL validation, progress parsing, throttling decisions, queue ordering, stale-result rejection, and state transitions. Player lifecycle tests use the fake downloader and avoid opening an audio device unless explicitly testing that boundary.

Integration-style tests exercise:

1. submit → fetching → queued → playing;
2. multiple submissions completing out of order but playing in submission order;
3. stop during download, with late results ignored;
4. shutdown during download, with worker cancellation;
5. missing downloader, timeout, malformed output, decode failure, and player-thread loss;
6. pause/next/stop feedback when no operation is possible;
7. TUI status updates without progress-message spam.

The final verification gate is formatting, clippy with warnings denied, workspace tests, and the repository `scripts/check.sh` command.

## Acceptance Criteria

- A submitted URL produces immediate visible fetching feedback.
- Download progress is visible when available and does not flood history.
- Playback order always matches submission order.
- Stop and shutdown cancel active downloads and prevent late playback.
- Repeated additions cannot create unbounded downloader processes or threads.
- Every user command either takes effect or produces clear feedback.
- Common failures are actionable and do not crash the TUI.
- Radio lifecycle tests run without network access, YouTube, or a real audio device.
- Existing commands and keyboard shortcuts remain compatible.
