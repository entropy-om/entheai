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

/// A downloaded or local ready-to-play track.
#[derive(Debug, Clone)]
pub struct Track {
    pub title: String,
    pub path: PathBuf,
}

/// Control messages from the UI to the player thread.
#[derive(Debug, Clone)]
pub enum Command {
    /// Download the audio of a URL (yt-dlp) or queue a local file.
    Add(String),
    /// Load procedural seed audio (defaults to `~/Downloads/Mesa*`) and start ambient playback.
    Seed(String),
    /// Toggle procedural audio fallback loop mode.
    Procedural(bool),
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
    /// yt-dlp started fetching a URL or loading a local track.
    Fetching {
        url: String,
    },
    /// A track finished downloading or local seeding and joined the queue.
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

/// Expand leading `~` to `$HOME` if present.
pub fn expand_tilde(path_str: &str) -> PathBuf {
    if let Some(rest) = path_str.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path_str)
}

/// Resolve local seed audio files matching a pattern or directory (defaults to `~/Downloads/Mesa*` & psychedelic/desert/stoner/space/metal tracks).
pub fn resolve_seed_files(pattern: &str) -> Vec<PathBuf> {
    let target = if pattern.trim().is_empty() {
        "~/Downloads/Mesa*"
    } else {
        pattern.trim()
    };
    let expanded = expand_tilde(target);

    if expanded.is_file() {
        return vec![expanded];
    }

    let mut found = Vec::new();

    // Check parent directory of target if pattern is a glob
    if let Some(parent) = expanded.parent() {
        let file_prefix = expanded.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let clean_prefix = file_prefix.trim_end_matches('*');

        if parent.exists() && parent.is_dir() {
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if !p.is_file() {
                        continue;
                    }
                    let name_lc = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    let ext = p
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if !matches!(ext.as_str(), "mp3" | "m4a" | "wav" | "flac" | "ogg") {
                        continue;
                    }

                    // Match explicitly specified prefix, or default genre keywords: mesa, desert, psychedelic, stoner, space, metal
                    let is_match = if !clean_prefix.is_empty() {
                        name_lc.starts_with(&clean_prefix.to_lowercase())
                    } else {
                        name_lc.contains("mesa")
                            || name_lc.contains("desert")
                            || name_lc.contains("psychedelic")
                            || name_lc.contains("stoner")
                            || name_lc.contains("space")
                            || name_lc.contains("metal")
                            || name_lc.contains("chillout")
                    };

                    if is_match {
                        found.push(p);
                    }
                }
            }
        }
    }

    // Fallback search in ~/.cache/entheai/radio/ if empty
    if found.is_empty() {
        let cache_dir = Radio::default_cache_dir();
        if cache_dir.exists() && cache_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(cache_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    let ext = p
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if p.is_file() && matches!(ext.as_str(), "mp3" | "m4a" | "wav" | "flac" | "ogg")
                    {
                        found.push(p);
                    }
                }
            }
        }
    }

    found.sort();
    found
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
    procedural_enabled: bool,
    procedural_seeds: Vec<PathBuf>,
    procedural_index: usize,
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
            if self.queue.is_empty() && !self.procedural_enabled {
                self.emit(Event::QueueEmpty);
            }
        }

        // Procedural loop fallback if queue is empty and procedural mode is enabled
        if self.queue.is_empty() && self.procedural_enabled && !self.procedural_seeds.is_empty() {
            // Pseudo-random track selection across seeds
            let idx = (self.procedural_index * 7 + 3) % self.procedural_seeds.len();
            let seed_path = &self.procedural_seeds[idx];
            self.procedural_index += 1;
            let title = seed_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Psychedelic Desert Metal Seed")
                .to_string();
            let track = Track {
                title: format!(
                    "♪ Procedural Psychedelic Metal: {title} (Variation #{})",
                    self.procedural_index
                ),
                path: seed_path.clone(),
            };
            self.queue.push_back(track);
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
            Command::Add(url_or_path) => {
                self.emit(Event::Fetching {
                    url: url_or_path.clone(),
                });
                let tx = dl_tx.clone();
                let dir = cache_dir.to_path_buf();
                let timeout = self.download_timeout;
                std::thread::Builder::new()
                    .name("entheai-radio-dl".into())
                    .spawn(move || {
                        let _ = tx.send(Msg::Downloaded(download(&url_or_path, &dir, timeout)));
                    })
                    .ok();
            }
            Command::Seed(pattern) => {
                let seeds = resolve_seed_files(&pattern);
                if seeds.is_empty() {
                    self.emit(Event::Error(format!(
                        "No procedural seed audio found for: {pattern}"
                    )));
                } else {
                    self.procedural_seeds = seeds.clone();
                    self.procedural_enabled = true;
                    self.emit(Event::Queued {
                        title: format!(
                            "Procedural Psychedelic Desert Metal Radio ({} seeds active)",
                            seeds.len()
                        ),
                    });
                    self.advance();
                }
            }
            Command::Procedural(enable) => {
                self.procedural_enabled = enable;
                if enable && self.procedural_seeds.is_empty() {
                    self.procedural_seeds = resolve_seed_files("~/Downloads/Mesa*");
                }
                self.advance();
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
                    sink.stop(); // drop current source
                }
                self.current = None;
                self.advance();
            }
            Command::Stop => {
                if let Some((_, sink)) = &self.audio {
                    sink.stop();
                }
                self.current = None;
                self.procedural_enabled = false;
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
        procedural_enabled: true, // Default to procedural mode out-of-the-box
        procedural_seeds: Vec::new(),
        procedural_index: 0,
    };

    // Auto-seed Mesa / Psychedelic / Desert / Stoner / Space / Metal downloads on startup
    let default_seeds = resolve_seed_files("~/Downloads/Mesa*");
    if !default_seeds.is_empty() {
        p.procedural_seeds = default_seeds;
    }

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
/// same command protocol so the UI never blocks.
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
            Msg::Cmd(Command::Add(url_or_path)) => {
                emit(Event::Fetching {
                    url: url_or_path.clone(),
                });
                let tx = dl_tx.clone();
                let dir = cache_dir.clone();
                std::thread::Builder::new()
                    .name("entheai-radio-dl".into())
                    .spawn(move || {
                        let _ = tx.send(Msg::Downloaded(download(&url_or_path, &dir, timeout)));
                    })
                    .ok();
            }
            Msg::Cmd(Command::Seed(pattern)) => {
                let seeds = resolve_seed_files(&pattern);
                emit(Event::Queued {
                    title: format!("Procedural Mesa Ambient ({} seeds active)", seeds.len()),
                });
            }
            Msg::Cmd(Command::Procedural(_)) => {}
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

/// Clamp a configured download timeout to a sane minimum.
fn download_timeout(secs: u64) -> Duration {
    Duration::from_secs(secs.max(1))
}

/// Run `yt-dlp` for URLs or load local audio files directly. Returns the track title + final file path.
fn download(url_or_path: &str, cache_dir: &Path, timeout: Duration) -> Result<Track, String> {
    let expanded = expand_tilde(url_or_path);
    if expanded.is_file() {
        let title = expanded
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Local Track")
            .to_string();
        return Ok(Track {
            title,
            path: expanded,
        });
    }

    if !(url_or_path.starts_with("http://") || url_or_path.starts_with("https://")) {
        let seeds = resolve_seed_files(url_or_path);
        if let Some(seed) = seeds.into_iter().next() {
            let title = seed
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Procedural Mesa Seed")
                .to_string();
            return Ok(Track { title, path: seed });
        }
        return Err(format!(
            "refusing non-http(s) URL or invalid local path: {url_or_path}"
        ));
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
        .arg(url_or_path)
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
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("yt-dlp timed out after {}s", timeout.as_secs()));
        }
    };

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

/// stdout carries one `title` line and one `after_move:filepath` line.
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

    #[test]
    fn resolve_seed_files_finds_mesa_downloads() {
        let seeds = resolve_seed_files("~/Downloads/Mesa*");
        if !seeds.is_empty() {
            assert!(seeds.iter().any(|p| p.to_str().unwrap().contains("Mesa")));
        }
    }
}
