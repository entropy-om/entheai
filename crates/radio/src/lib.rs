//! In-TUI procedural music: one bundled track — "Standing-Onde" by
//! 8bit-Wraith (<https://soundcloud.com/8bit-wraith/standing-onde>), embedded
//! at compile time and looped through the default output device with
//! `rodio`. No network fetch, no external tool, no user-supplied URL or
//! local-file seed — this is the entheai radio, nothing else.
//!
//! Architecture: [`Radio::spawn`] starts one dedicated OS thread that owns the
//! audio stack (`rodio::OutputStream` is `!Send`, so it can never live on a
//! tokio worker). The UI talks to it through a std mpsc [`Command`] channel and
//! listens on a tokio unbounded [`Event`] channel (async-recv friendly for
//! `select!`).
//!
//! The audio device is opened lazily on first play, so constructing a `Radio`
//! is free and headless environments (CI, tests) only error when they actually
//! try to make sound.

#[cfg(feature = "audio")]
use std::io::Cursor;
use std::sync::mpsc as std_mpsc;
#[cfg(feature = "audio")]
use std::time::Duration;

use tokio::sync::mpsc as tokio_mpsc;

/// The one track this radio ever plays, embedded at compile time so playback
/// needs no network access, no cache directory, and no external tool.
#[cfg(feature = "audio")]
const TRACK_BYTES: &[u8] = include_bytes!("../assets/standing-onde.mp3");
#[cfg(feature = "audio")]
const TRACK_TITLE: &str = "Standing-Onde — 8bit-Wraith";

/// Control messages from the UI to the player thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Toggle pause/resume of playback.
    TogglePause,
    /// Restart the track from the beginning; also re-enables the loop if a
    /// prior `Stop` had disabled it.
    Next,
    /// Stop playback; the loop won't restart until `Next` is sent again.
    Stop,
    /// Shut the player thread down.
    Shutdown,
}

/// Progress notifications from the player thread to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The track (re)started playing; `loop_count` is 1 on first play and
    /// increments every time the loop restarts.
    NowPlaying {
        title: String,
        loop_count: u32,
    },
    Paused,
    Resumed,
    Stopped,
    Error(String),
}

/// Handle to the player thread. Dropping it shuts the thread down.
pub struct Radio {
    cmd_tx: std_mpsc::Sender<Command>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
    /// Kept alive only so `events` never observes a closed channel; unused
    /// otherwise (`None` for a real [`Radio::spawn`], where the player
    /// thread owns the sender instead).
    _event_tx_keepalive: Option<tokio_mpsc::UnboundedSender<Event>>,
}

impl Radio {
    /// Start the player thread.
    pub fn spawn() -> Result<Radio, std::io::Error> {
        let (cmd_tx, cmd_rx) = std_mpsc::channel::<Command>();
        let (event_tx, events) = tokio_mpsc::unbounded_channel::<Event>();
        std::thread::Builder::new()
            .name("entheai-radio".into())
            .spawn(move || player_thread(cmd_rx, event_tx))?;
        Ok(Radio {
            cmd_tx,
            events,
            _event_tx_keepalive: None,
        })
    }

    /// Build a no-op radio stub that accepts commands but does nothing. All
    /// `send()` calls are silently dropped, and `next_event()` returns `None`
    /// (pending forever). Useful as a fallback when `spawn` fails.
    pub fn noop() -> Radio {
        let (cmd_tx, _cmd_rx) = std_mpsc::channel::<Command>();
        let (event_tx, events) = tokio_mpsc::unbounded_channel::<Event>();
        Radio {
            cmd_tx,
            events,
            _event_tx_keepalive: Some(event_tx),
        }
    }

    /// Send a control command. Errors are ignored: if the player thread is
    /// gone there is nobody left to control.
    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Await the next player event (cancel-safe; usable in `tokio::select!`).
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.recv().await
    }
}

impl Drop for Radio {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
    }
}

/// Everything the player thread mutates, bundled so handlers stay small.
#[cfg(feature = "audio")]
struct Player {
    /// Audio device + sink, opened lazily on first play.
    audio: Option<(rodio::OutputStream, rodio::Sink)>,
    /// Whether the loop should (re)start when the current playthrough ends.
    enabled: bool,
    /// Whether a playthrough is currently loaded into the sink.
    playing: bool,
    loop_count: u32,
    events: tokio_mpsc::UnboundedSender<Event>,
}

#[cfg(feature = "audio")]
impl Player {
    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    /// Get (opening if needed) the sink. `None` + an `Error` event if the
    /// default output device can't be opened.
    fn sink(&mut self) -> Option<&rodio::Sink> {
        if self.audio.is_none() {
            match rodio::OutputStream::try_default() {
                Ok((stream, handle)) => match rodio::Sink::try_new(&handle) {
                    Ok(sink) => self.audio = Some((stream, sink)),
                    Err(e) => self.emit(Event::Error(format!("audio sink: {e}"))),
                },
                Err(e) => self.emit(Event::Error(format!("audio device: {e}"))),
            }
        }
        self.audio.as_ref().map(|(_, sink)| sink)
    }

    /// If enabled and the current playthrough (if any) has ended, loop it.
    fn advance(&mut self) {
        if !self.enabled {
            return;
        }
        if self.playing {
            if let Some((_, sink)) = &self.audio {
                if !sink.empty() {
                    return; // still playing
                }
            }
            self.playing = false; // playthrough ended naturally
        }
        self.restart();
    }

    /// Decode the embedded track from scratch and start it playing.
    fn restart(&mut self) {
        let source = match rodio::Decoder::new(Cursor::new(TRACK_BYTES)) {
            Ok(s) => s,
            Err(e) => {
                self.emit(Event::Error(format!("decode embedded track: {e}")));
                return;
            }
        };
        self.loop_count += 1;
        if let Some(sink) = self.sink() {
            sink.stop();
            sink.append(source);
            sink.play();
            self.playing = true;
            self.emit(Event::NowPlaying {
                title: TRACK_TITLE.to_string(),
                loop_count: self.loop_count,
            });
        }
    }

    fn handle(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::TogglePause => {
                if let Some((_, sink)) = &self.audio {
                    if sink.is_paused() {
                        sink.play();
                        self.emit(Event::Resumed);
                    } else if self.playing {
                        sink.pause();
                        self.emit(Event::Paused);
                    }
                }
            }
            Command::Next => {
                self.enabled = true;
                self.playing = false;
                self.restart();
            }
            Command::Stop => {
                if let Some((_, sink)) = &self.audio {
                    sink.stop();
                }
                self.playing = false;
                self.enabled = false;
                self.emit(Event::Stopped);
            }
            Command::Shutdown => return false,
        }
        true
    }
}

#[cfg(feature = "audio")]
fn player_thread(rx: std_mpsc::Receiver<Command>, events: tokio_mpsc::UnboundedSender<Event>) {
    let mut p = Player {
        audio: None,
        enabled: true, // Default to procedural playback out-of-the-box
        playing: false,
        loop_count: 0,
        events,
    };
    loop {
        // Tick at 200ms so a finished playthrough is noticed promptly.
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(cmd) => {
                if !p.handle(cmd) {
                    return;
                }
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => p.advance(),
            Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Audio-disabled stub player (built when the `audio` feature is off). Keeps the
/// same command protocol so the UI never blocks.
#[cfg(not(feature = "audio"))]
fn player_thread(rx: std_mpsc::Receiver<Command>, events: tokio_mpsc::UnboundedSender<Event>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Stop => {
                let _ = events.send(Event::Stopped);
            }
            Command::Shutdown => return,
            Command::TogglePause | Command::Next => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "audio")]
    #[test]
    fn track_bytes_embedded_and_nonempty() {
        assert!(!TRACK_BYTES.is_empty());
    }

    #[tokio::test]
    async fn spawn_stop_emits_stopped_and_shuts_down() {
        let mut radio = Radio::spawn().unwrap();
        radio.send(Command::Stop);
        assert_eq!(radio.next_event().await, Some(Event::Stopped));
    }

    #[tokio::test]
    async fn noop_radio_never_emits() {
        let mut radio = Radio::noop();
        radio.send(Command::Next);
        let timeout = tokio::time::timeout(Duration::from_millis(50), radio.next_event()).await;
        assert!(timeout.is_err(), "noop radio must never emit an event");
    }
}
