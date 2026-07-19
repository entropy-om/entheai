use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::{Tool, ToolError};

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
        let fut = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true) // on timeout the future is dropped — reap the child, don't orphan it
            .output();
        let output = match tokio::time::timeout(Duration::from_secs(self.timeout_secs), fut).await {
            Ok(res) => res?,
            Err(_) => {
                return Err(ToolError::Timeout {
                    secs: self.timeout_secs,
                    command: command.to_string(),
                })
            }
        };
        let mut out = String::new();
        out.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            out.push_str("\n[stderr]\n");
            out.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        out.push_str(&format!("\n[exit: {}]", output.status.code().unwrap_or(-1)));
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
}
