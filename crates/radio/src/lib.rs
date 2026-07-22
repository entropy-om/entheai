//! In-TUI music radio: download YouTube audio with `yt-dlp`, queue it, and play
//! it through the default output device with `rodio`.
//!
//! Architecture: [`Radio::spawn`] starts one dedicated OS thread that owns the
//! audio stack (`rodio::OutputStream` is `!Send`, so it can never live on a
//! tokio worker). The UI talks to it through a std mpsc [`Command`] channel and
//! listens on a tokio unbounded [`Event`] channel (async-recv friendly for
//! `select!`). Each `Add` spawns a short-lived downloader thread running
//! `yt-dlp`; the finished track is fed back to the player thread and queued.
//!
//! The audio device is opened lazily on first play, so constructing a `Radio`
//! is free and headless environments (CI, tests) only error when they actually
//! try to make sound.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use tokio::sync::mpsc as tokio_mpsc;
use wait_timeout::ChildExt;

/// A downloaded, ready-to-play track.
#[derive(Debug, Clone)]
pub struct Track {
    pub title: String,
    pub path: PathBuf,
}

/// Control messages from the UI to the player thread.
#[derive(Debug, Clone)]
pub enum Command {
    /// Download the audio of a URL (yt-dlp) and enqueue it.
    Add(String),
    /// Toggle pause/resume of the current track.
    TogglePause,
    /// Skip the current track.
    Next,
    /// Stop playback and clear the queue.
    Stop,
    /// Shut the player thread down.
    Shutdown,
}

/// Progress notifications from the player thread to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// yt-dlp started fetching a URL.
    Fetching {
        url: String,
    },
    /// A track finished downloading and joined the queue.
    Queued {
        title: String,
    },
    /// A track started playing.
    NowPlaying {
        title: String,
    },
    Paused,
    Resumed,
    /// Playback stopped and the queue was cleared.
    Stopped,
    /// The queue ran dry after the last track ended.
    QueueEmpty,
    Error(String),
}

/// Handle to the player thread. Dropping it shuts the thread down.
pub struct Radio {
    cmd_tx: std_mpsc::Sender<Msg>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
}

/// Internal player-thread inbox: UI commands plus downloader results.
enum Msg {
    Cmd(Command),
    Downloaded(Result<Track, String>),
}

impl Radio {
    /// Start the player thread. `cache_dir` is where yt-dlp keeps audio files
    /// (created on demand; downloads are keyed by video id, so repeats are
    /// served from cache by yt-dlp itself). `download_timeout_secs` bounds a
    /// single `yt-dlp` invocation (see [`download_timeout`]); values below 1
    /// are floored to 1 second.
    pub fn spawn(cache_dir: PathBuf, download_timeout_secs: u64) -> Result<Radio, std::io::Error> {
        let (cmd_tx, cmd_rx) = std_mpsc::channel::<Msg>();
        let (event_tx, events) = tokio_mpsc::unbounded_channel::<Event>();
        let dl_tx = cmd_tx.clone();
        std::thread::Builder::new()
            .name("entheai-radio".into())
            .spawn(move || {
                player_thread(cmd_rx, dl_tx, event_tx, cache_dir, download_timeout_secs)
            })?;
        Ok(Radio { cmd_tx, events })
    }

    /// Build a no-op radio stub that accepts commands but does nothing. All
    /// `send()` calls are silently dropped, and `next_event()` returns `None`
    /// (pending forever). Useful as a fallback when `spawn` fails.
    pub fn noop() -> Radio {
        let (cmd_tx, _cmd_rx) = std_mpsc::channel::<Msg>();
        let (_event_tx, events) = tokio_mpsc::unbounded_channel::<Event>();
        Radio { cmd_tx, events }
    }

    /// Default cache dir: `~/.cache/entheai/radio` (temp dir fallback).
    pub fn default_cache_dir() -> PathBuf {
        match std::env::var_os("HOME") {
            Some(home) => Path::new(&home).join(".cache/entheai/radio"),
            None => std::env::temp_dir().join("entheai-radio"),
        }
    }

    /// Send a control command. Errors are ignored: if the player thread is
    /// gone there is nobody left to control.
    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(Msg::Cmd(cmd));
    }

    /// Await the next player event (cancel-safe; usable in `tokio::select!`).
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.recv().await
    }
}

impl Drop for Radio {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Msg::Cmd(Command::Shutdown));
    }
}

/// Everything the player thread mutates, bundled so handlers stay small.
#[cfg(feature = "audio")]
struct Player {
    /// Audio device + sink, opened lazily on first play.
    audio: Option<(rodio::OutputStream, rodio::Sink)>,
    queue: VecDeque<Track>,
    current: Option<Track>,
    events: tokio_mpsc::UnboundedSender<Event>,
    /// Hard ceiling on a single `yt-dlp` invocation, from config.
    download_timeout: Duration,
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

    /// If nothing is playing and the queue has a track, start it.
    fn advance(&mut self) {
        if self.current.is_some() {
            if let Some((_, sink)) = &self.audio {
                if !sink.empty() {
                    return; // still playing
                }
            }
            self.current = None; // track ended naturally
            if self.queue.is_empty() {
                self.emit(Event::QueueEmpty);
            }
        }
        let Some(track) = self.queue.pop_front() else {
            return;
        };
        let source = File::open(&track.path)
            .map_err(|e| format!("open {}: {e}", track.path.display()))
            .and_then(|f| {
                rodio::Decoder::new(BufReader::new(f)).map_err(|e| format!("decode: {e}"))
            });
        let source = match source {
            Ok(s) => s,
            Err(e) => {
                self.emit(Event::Error(format!("{}: {e}", track.title)));
                return;
            }
        };
        if let Some(sink) = self.sink() {
            sink.append(source);
            sink.play();
            self.emit(Event::NowPlaying {
                title: track.title.clone(),
            });
            self.current = Some(track);
        }
    }

    fn handle(&mut self, cmd: Command, dl_tx: &std_mpsc::Sender<Msg>, cache_dir: &Path) -> bool {
        match cmd {
            Command::Add(url) => {
                self.emit(Event::Fetching { url: url.clone() });
                let tx = dl_tx.clone();
                let dir = cache_dir.to_path_buf();
                let timeout = self.download_timeout;
                std::thread::Builder::new()
                    .name("entheai-radio-dl".into())
                    .spawn(move || {
                        let _ = tx.send(Msg::Downloaded(download(&url, &dir, timeout)));
                    })
                    .ok();
            }
            Command::TogglePause => {
                if let Some((_, sink)) = &self.audio {
                    if sink.is_paused() {
                        sink.play();
                        self.emit(Event::Resumed);
                    } else if self.current.is_some() {
                        sink.pause();
                        self.emit(Event::Paused);
                    }
                }
            }
            Command::Next => {
                if let Some((_, sink)) = &self.audio {
                    sink.stop(); // drop the current source; advance() starts the next
                }
                self.current = None;
                self.advance();
            }
            Command::Stop => {
                if let Some((_, sink)) = &self.audio {
                    sink.stop();
                }
                self.current = None;
                self.queue.clear();
                self.emit(Event::Stopped);
            }
            Command::Shutdown => return false,
        }
        true
    }
}

#[cfg(feature = "audio")]
fn player_thread(
    rx: std_mpsc::Receiver<Msg>,
    dl_tx: std_mpsc::Sender<Msg>,
    events: tokio_mpsc::UnboundedSender<Event>,
    cache_dir: PathBuf,
    download_timeout_secs: u64,
) {
    let mut p = Player {
        audio: None,
        queue: VecDeque::new(),
        current: None,
        events,
        download_timeout: download_timeout(download_timeout_secs),
    };
    loop {
        // Tick at 200ms so track-end (sink drained) is noticed promptly.
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Msg::Cmd(cmd)) => {
                if !p.handle(cmd, &dl_tx, &cache_dir) {
                    return;
                }
            }
            Ok(Msg::Downloaded(Ok(track))) => {
                p.emit(Event::Queued {
                    title: track.title.clone(),
                });
                p.queue.push_back(track);
                p.advance();
            }
            Ok(Msg::Downloaded(Err(e))) => p.emit(Event::Error(e)),
            Err(std_mpsc::RecvTimeoutError::Timeout) => p.advance(),
            Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Audio-disabled stub player (built when the `audio` feature is off). Keeps the
/// same command protocol so the UI never blocks: `Add` still downloads + queues
/// (yt-dlp is audio-free), but playback is a silent no-op because this build
/// links no audio backend.
#[cfg(not(feature = "audio"))]
fn player_thread(
    rx: std_mpsc::Receiver<Msg>,
    dl_tx: std_mpsc::Sender<Msg>,
    events: tokio_mpsc::UnboundedSender<Event>,
    cache_dir: PathBuf,
    download_timeout_secs: u64,
) {
    let timeout = download_timeout(download_timeout_secs);
    let mut queue: VecDeque<Track> = VecDeque::new();
    let emit = |ev: Event| {
        let _ = events.send(ev);
    };
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Cmd(Command::Add(url)) => {
                emit(Event::Fetching { url: url.clone() });
                let tx = dl_tx.clone();
                let dir = cache_dir.clone();
                std::thread::Builder::new()
                    .name("entheai-radio-dl".into())
                    .spawn(move || {
                        let _ = tx.send(Msg::Downloaded(download(&url, &dir, timeout)));
                    })
                    .ok();
            }
            Msg::Cmd(Command::Stop) => {
                queue.clear();
                emit(Event::Stopped);
            }
            Msg::Cmd(Command::TogglePause) | Msg::Cmd(Command::Next) => {}
            Msg::Cmd(Command::Shutdown) => return,
            Msg::Downloaded(Ok(track)) => {
                emit(Event::Queued {
                    title: track.title.clone(),
                });
                queue.push_back(track);
            }
            Msg::Downloaded(Err(e)) => emit(Event::Error(e)),
        }
    }
}

/// Clamp a configured download timeout to a sane minimum. Without a ceiling,
/// a hung/slow `yt-dlp` process blocks the downloader thread (and keeps a
/// live child process) forever — repeated `/radio add`s would pile both up
/// with no way to cancel. A `0` (or misconfigured) value would disable the
/// timeout outright, so floor it at 1 second instead.
fn download_timeout(secs: u64) -> Duration {
    Duration::from_secs(secs.max(1))
}

/// Run `yt-dlp`, extracting audio as m4a into `cache_dir`. Returns the track
/// title + final file path. Blocking; runs on a downloader thread. Bounded by
/// `timeout`: past that, the child is killed and reaped.
fn download(url: &str, cache_dir: &Path, timeout: Duration) -> Result<Track, String> {
    // Security: `url` is handed to yt-dlp's argv parser. Reject anything that
    // isn't a plain http(s) URL — a `-`-prefixed value is parsed as a FLAG
    // (e.g. `--exec=…` runs an arbitrary command). This is the single choke
    // point for every caller (`/radio <url>` and `/radio add <url>`).
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("refusing non-http(s) URL: {url}"));
    }
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("cache dir: {e}"))?;
    let mut child = Process::new("yt-dlp")
        .args([
            "--no-playlist",
            "-f",
            "bestaudio",
            "-x",
            "--audio-format",
            "m4a",
            "-o",
        ])
        .arg(cache_dir.join("%(id)s.%(ext)s"))
        .args([
            "--print",
            "title",
            "--print",
            "after_move:filepath",
            "--no-simulate",
            "--quiet",
        ])
        .arg("--")
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("yt-dlp not runnable ({e}); install with `brew install yt-dlp`"))?;

    let status = match child
        .wait_timeout(timeout)
        .map_err(|e| format!("yt-dlp wait failed: {e}"))?
    {
        Some(status) => status,
        None => {
            // Timed out: the process has already run long past a sane single
            // download, so kill and reap it rather than leaving it running.
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("yt-dlp timed out after {}s", timeout.as_secs()));
        }
    };

    // The process has exited (wait_timeout returned Some), so its pipes are
    // closed and draining them to completion here cannot deadlock.
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }

    if !status.success() {
        return Err(format!(
            "yt-dlp failed: {}",
            stderr.lines().last().unwrap_or("unknown error")
        ));
    }
    parse_ytdlp_output(&stdout)
}

/// stdout carries one `title` line (pre-download) and one `after_move:filepath`
/// line (post-download). The filepath is the last line that looks like a path
/// into the cache; the title is the first line that is not the path.
fn parse_ytdlp_output(stdout: &str) -> Result<Track, String> {
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    let path = lines
        .iter()
        .rev()
        .find(|l| Path::new(l).is_absolute() || l.ends_with(".m4a"))
        .ok_or_else(|| "yt-dlp printed no file path".to_string())?;
    let title = lines
        .iter()
        .find(|l| *l != path)
        .copied()
        .unwrap_or("unknown title");
    Ok(Track {
        title: title.to_string(),
        path: PathBuf::from(*path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_title_then_path() {
        let t = parse_ytdlp_output("Some Song\n/tmp/cache/abc123.m4a\n").unwrap();
        assert_eq!(t.title, "Some Song");
        assert_eq!(t.path, PathBuf::from("/tmp/cache/abc123.m4a"));
    }

    #[test]
    fn parse_output_relative_m4a() {
        let t = parse_ytdlp_output("Tune\ncache/abc.m4a\n").unwrap();
        assert_eq!(t.title, "Tune");
        assert_eq!(t.path, PathBuf::from("cache/abc.m4a"));
    }

    #[test]
    fn parse_output_no_path_errors() {
        assert!(parse_ytdlp_output("just a title\n").is_err());
        assert!(parse_ytdlp_output("").is_err());
    }

    #[test]
    fn parse_output_path_only_falls_back_title() {
        let t = parse_ytdlp_output("/tmp/x.m4a\n").unwrap();
        assert_eq!(t.title, "unknown title");
    }

    #[test]
    fn default_cache_dir_is_under_home_or_tmp() {
        let d = Radio::default_cache_dir();
        assert!(d.ends_with("radio") || d.ends_with("entheai-radio"));
    }

    #[tokio::test]
    async fn spawn_stop_emits_stopped_and_shuts_down() {
        let mut radio = Radio::spawn(std::env::temp_dir().join("entheai-radio-test"), 300).unwrap();
        radio.send(Command::Stop);
        assert_eq!(radio.next_event().await, Some(Event::Stopped));
    }

    #[test]
    fn download_timeout_floors_at_one_second() {
        assert_eq!(download_timeout(300), Duration::from_secs(300));
        assert_eq!(download_timeout(0), Duration::from_secs(1));
    }
}
