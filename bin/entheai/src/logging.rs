//! Minimal, TUI-safe `log` backend for entheai.
//!
//! Routing (chosen 2026-07-20): an interactive TUI session logs to a FILE only
//! (`~/.cache/entheai/entheai.log`) so the alternate-screen is never corrupted;
//! one-shot / `--fanout` / `--memory` runs additionally mirror to stderr (stdout
//! carries the answer, so stderr is safe). Level defaults to `warn`, overridable
//! via `ENTHEAI_LOG` / `RUST_LOG` (a bare level name: error|warn|info|debug|trace|off).
//! Best-effort: any setup failure degrades quietly and never panics.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

struct EntheaiLogger {
    level: LevelFilter,
    file: Option<Mutex<File>>,
    stderr: bool,
}

impl Log for EntheaiLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {}: {}\n",
            utc_hms(),
            record.level(),
            record.target(),
            record.args()
        );
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.write_all(line.as_bytes());
            }
        }
        if self.stderr {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.flush();
            }
        }
    }
}

/// UTC time-of-day `HH:MM:SS` — enough to correlate log lines without pulling in
/// a datetime crate.
fn utc_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// Map a bare level name to a `LevelFilter`, defaulting to `warn`. (env_logger's
/// per-module filter syntax is intentionally NOT supported — a bare level only.)
fn parse_level(raw: &str) -> LevelFilter {
    match raw.trim().to_ascii_lowercase().as_str() {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        "off" => LevelFilter::Off,
        _ => LevelFilter::Warn,
    }
}

fn level_from_env() -> LevelFilter {
    let raw = std::env::var("ENTHEAI_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_default();
    parse_level(&raw)
}

/// Open (append) `~/.cache/entheai/entheai.log`, creating the parent dir.
fn open_log_file() -> Option<File> {
    let home = std::env::var("HOME").ok()?;
    let dir = std::path::Path::new(&home).join(".cache").join("entheai");
    std::fs::create_dir_all(&dir).ok()?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("entheai.log"))
        .ok()
}

/// Install the global logger. `interactive` = an interactive TUI session (logs
/// to the file only); otherwise stderr is mirrored too. Idempotent-safe: a
/// second call — or a logger already installed — is ignored. Never panics.
pub fn init(interactive: bool) {
    let level = level_from_env();
    let logger = EntheaiLogger {
        level,
        file: open_log_file().map(Mutex::new),
        stderr: !interactive,
    };
    // `set_boxed_logger` errors only if a logger is already set — ignore it.
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_levels_and_defaults_to_warn() {
        assert_eq!(parse_level("debug"), LevelFilter::Debug);
        assert_eq!(parse_level("WARN"), LevelFilter::Warn);
        assert_eq!(parse_level(" info "), LevelFilter::Info);
        assert_eq!(parse_level("off"), LevelFilter::Off);
        assert_eq!(
            parse_level("entheai=debug"),
            LevelFilter::Warn,
            "module filters unsupported → default"
        );
        assert_eq!(parse_level("garbage"), LevelFilter::Warn);
        assert_eq!(parse_level(""), LevelFilter::Warn);
    }

    #[test]
    fn hms_is_wellformed() {
        let t = utc_hms();
        assert_eq!(t.len(), 8, "HH:MM:SS");
        assert_eq!(t.as_bytes()[2], b':');
        assert_eq!(t.as_bytes()[5], b':');
    }
}
