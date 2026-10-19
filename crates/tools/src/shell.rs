use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

use crate::{Tool, ToolError};

/// Size of each individual read() call against the child's stdout/stderr pipes.
const READ_CHUNK: usize = 8192;

/// SIGKILL the whole process group `pid` (its own, via `.process_group(0)`
/// at spawn — negative pid targets the group, not just `pid` itself) so a
/// backgrounded grandchild (`cmd &`, a pipeline, an exec'd server) dies with
/// `/bin/sh`, instead of surviving as an orphan holding the closed-parent
/// stdout/stderr pipe's write end open. `None`/kill failure is not fatal —
/// `child.start_kill()` right after this still reaps `/bin/sh` itself.
fn kill_process_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: `kill` with a negative pid signals the process group;
        // no memory/pointers involved, an ESRCH (already-dead group) is
        // harmless and intentionally ignored.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

pub struct RunShell {
    cwd: PathBuf,
    timeout_secs: u64,
    output_cap: usize,
}
impl RunShell {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            timeout_secs: 120,
            output_cap: 100_000,
        }
    }
    /// Override the command timeout (seconds) and combined-output byte cap.
    pub fn with_limits(mut self, timeout_secs: u64, output_cap: usize) -> Self {
        self.timeout_secs = timeout_secs.max(1);
        self.output_cap = output_cap;
        self
    }
}
#[async_trait]
impl Tool for RunShell {
    fn name(&self) -> &str {
        "run_shell"
    }
    fn tier(&self) -> entheai_permission::Tier {
        entheai_permission::Tier::Exec
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Run a shell command in the workspace directory and return combined stdout/stderr.",
                "parameters": {
                    "type": "object",
                    "properties": { "command": { "type": "string", "description": "The shell command line to run." } },
                    "required": ["command"]
                }
            }
        })
    }
    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| ToolError::MissingArg("command".into()))?;

        // Spawn with piped stdio instead of `.output()`: `.output()` buffers the
        // child's entire stdout+stderr into memory before we ever get to apply
        // `output_cap`, so a runaway command (`yes`, `cat /dev/zero`, ...) can OOM
        // the agent well before the timeout fires. Reading through a bounded loop
        // below keeps memory use capped to `output_cap` for the life of the call.
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true); // backstop: reap the child if we return early without an explicit kill
                                 // Its own process group: `start_kill()` SIGKILLs only `/bin/sh`
                                 // itself, so a backgrounded grandchild (`cmd &`, a pipeline, an
                                 // exec'd server) survived on timeout/cap, kept the stdout/stderr
                                 // pipes' write end open, and leaked as an orphaned process. Killing
                                 // the whole group below (`kill_process_group`) reaches it too.
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn()?;
        let pid = child.id();

        let mut stdout = child.stdout.take().expect("stdout piped");
        let mut stderr = child.stderr.take().expect("stderr piped");
        let cap = self.output_cap;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(self.timeout_secs);

        let mut stdout_buf: Vec<u8> = Vec::new();
        let mut stderr_buf: Vec<u8> = Vec::new();
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut chunk_out = [0u8; READ_CHUNK];
        let mut chunk_err = [0u8; READ_CHUNK];
        let mut timed_out = false;

        while stdout_open || stderr_open {
            let used = stdout_buf.len() + stderr_buf.len();
            if used >= cap {
                break; // combined byte budget exhausted — stop reading, kill below
            }
            let take = (cap - used).min(READ_CHUNK);
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    timed_out = true;
                    break;
                }
                res = stdout.read(&mut chunk_out[..take]), if stdout_open => {
                    match res {
                        Ok(0) | Err(_) => stdout_open = false,
                        Ok(n) => stdout_buf.extend_from_slice(&chunk_out[..n]),
                    }
                }
                res = stderr.read(&mut chunk_err[..take]), if stderr_open => {
                    match res {
                        Ok(0) | Err(_) => stderr_open = false,
                        Ok(n) => stderr_buf.extend_from_slice(&chunk_err[..n]),
                    }
                }
            }
        }

        if timed_out {
            kill_process_group(pid);
            let _ = child.start_kill();
            let _ = child.wait().await; // reap — never leave a zombie
            return Err(ToolError::Timeout {
                secs: self.timeout_secs,
                command: command.to_string(),
            });
        }

        let capped = stdout_buf.len() + stderr_buf.len() >= cap;
        if capped {
            // Runaway output: stop the child (and its group) now instead of
            // waiting it out.
            kill_process_group(pid);
            let _ = child.start_kill();
        }
        let status = child.wait().await?; // reap — never leave a zombie

        let mut out = String::new();
        out.push_str(&String::from_utf8_lossy(&stdout_buf));
        if !stderr_buf.is_empty() {
            out.push_str("\n[stderr]\n");
            out.push_str(&String::from_utf8_lossy(&stderr_buf));
        }
        out.push_str(&format!("\n[exit: {}]", status.code().unwrap_or(-1)));
        let max = self.output_cap;
        if out.len() > max {
            let mut end = max;
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            out.push_str(&format!("\n...[output truncated at {max} bytes]"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shell_honors_configured_timeout() {
        let dir = std::env::temp_dir();
        let sh = RunShell::new(&dir).with_limits(1, 100_000); // 1s timeout
        let err = sh
            .call(serde_json::json!({"command": "sleep 3"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Timeout { secs: 1, .. }));
    }

    /// A flooding command (`yes`) produces far more than `output_cap` bytes.
    /// The bounded reader must stop and kill the child at the cap — not buffer
    /// everything in memory and not wait out the (deliberately huge) timeout.
    #[tokio::test]
    async fn run_shell_bounds_flooding_output_and_returns_promptly() {
        let dir = std::env::temp_dir();
        let cap = 1_000usize;
        // Timeout is far longer than any sane test run; if the cap-kill path
        // didn't work, this test would hang until the timeout instead.
        let sh = RunShell::new(&dir).with_limits(120, cap);
        let start = std::time::Instant::now();
        let out = sh
            .call(serde_json::json!({"command": "yes"}))
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(10),
            "expected the flooding child to be killed promptly at the cap, took {elapsed:?}"
        );
        // Content is bounded to ~cap bytes (truncation marker adds a small, fixed overhead).
        assert!(
            out.len() <= cap + 200,
            "expected output bounded near cap ({cap}), got {} bytes",
            out.len()
        );
        assert!(
            out.contains(&format!("truncated at {cap} bytes")),
            "expected truncation marker in output: {out:?}"
        );
    }

    /// `start_kill()` alone only SIGKILLs `/bin/sh`; a backgrounded
    /// grandchild survives, keeps running, and leaks. Killing the whole
    /// process group must reach it too.
    #[tokio::test]
    async fn timeout_kills_a_backgrounded_grandchild_not_just_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat = dir.path().join("heartbeat");
        std::fs::write(&heartbeat, "").unwrap();
        let sh = RunShell::new(dir.path()).with_limits(1, 100_000); // 1s timeout
        let cmd = format!(
            "(while true; do echo x >> '{}'; sleep 0.05; done) & sleep 5",
            heartbeat.display()
        );
        let err = sh
            .call(serde_json::json!({"command": cmd}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Timeout { .. }));

        let size_at_timeout = std::fs::metadata(&heartbeat).map(|m| m.len()).unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let size_after_wait = std::fs::metadata(&heartbeat).map(|m| m.len()).unwrap_or(0);
        assert_eq!(
            size_at_timeout, size_after_wait,
            "the backgrounded grandchild kept writing after timeout — it was not killed"
        );
    }
}
