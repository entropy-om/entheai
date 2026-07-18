use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::Tool;

pub struct RunShell {
    cwd: PathBuf,
}
impl RunShell {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
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
    async fn call(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing string arg 'command'"))?;
        let fut = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true) // on timeout the future is dropped — reap the child, don't orphan it
            .output();
        let output = match tokio::time::timeout(Duration::from_secs(120), fut).await {
            Ok(res) => res?,
            Err(_) => anyhow::bail!("command timed out after 120s: {command}"),
        };
        let mut out = String::new();
        out.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            out.push_str("\n[stderr]\n");
            out.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        out.push_str(&format!("\n[exit: {}]", output.status.code().unwrap_or(-1)));
        const MAX: usize = 100_000;
        if out.len() > MAX {
            let mut end = MAX;
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            out.push_str("\n...[output truncated at 100000 bytes]");
        }
        Ok(out)
    }
}
