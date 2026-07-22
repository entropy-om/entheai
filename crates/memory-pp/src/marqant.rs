//! Stage 3 — deterministic compression seam (marqant `mq`). Slice-1 stub is an
//! identity passthrough (never reached on the live path — StubMesh short-circuits
//! before it). Slice 2 swaps in the `mq compress <in.md> -o <out.mq> --semantic`
//! subprocess (file-arg I/O, `--semantic` yes / `--binary` no, capped reader +
//! timeout mirroring crates/tools/src/shell.rs; deterministic, golden-testable).

use std::time::Duration;

use async_trait::async_trait;

use crate::error::PpError;

#[async_trait]
pub trait Marqant: Send + Sync {
    /// Deterministically distil raw findings into the injectable brief.
    /// `deadline` bounds the Slice-2 `mq` subprocess (the impl owns `kill_on_drop`)
    /// — symmetric with `MeshSearch::rerank` so Slice 2 drops in without a trait
    /// change or orphaned `mq` processes on timeout.
    async fn compress(&self, findings: &str, deadline: Duration) -> Result<String, PpError>;
}

pub struct StubMarqant;

#[async_trait]
impl Marqant for StubMarqant {
    async fn compress(&self, findings: &str, _deadline: Duration) -> Result<String, PpError> {
        Ok(findings.to_string())
    }
}

/// Cap on bytes read back from the compressor's output file.
const MAX_MARQANT_OUTPUT: u64 = 1 << 20; // 1 MiB

/// Slice-2 production compressor: shells out to marqant (`mq compress <in.md> -o
/// <out.mq> --semantic`) with file-arg I/O, bounded by the deadline, `kill_on_drop`
/// so a slow `mq` can't orphan. Deterministic and model-free (golden-testable). Any
/// failure (missing `mq`, non-zero exit, timeout, unreadable output) is an `Err`,
/// so the processor falls back to top-K rather than injecting a bad brief.
pub struct SubprocessMarqant {
    program: String,
    prefix: Vec<String>,
}

impl SubprocessMarqant {
    /// `cmd` is whitespace-split into program + leading args (e.g. `"mq"` or
    /// `"python -m marqant"`); `compress <in> -o <out> --semantic` is appended.
    pub fn new(cmd: &str) -> Self {
        let mut parts = cmd.split_whitespace().map(str::to_string);
        let program = parts.next().unwrap_or_default();
        let prefix: Vec<String> = parts.collect();
        SubprocessMarqant { program, prefix }
    }
}

#[async_trait]
impl Marqant for SubprocessMarqant {
    async fn compress(&self, findings: &str, deadline: Duration) -> Result<String, PpError> {
        if self.program.is_empty() {
            return Err(PpError::Marqant("empty marqant_cmd".into()));
        }
        match tokio::time::timeout(deadline, run_marqant(&self.program, &self.prefix, findings)).await
        {
            Ok(r) => r,
            Err(_) => Err(PpError::Marqant("mq deadline exceeded".into())),
        }
    }
}

/// Write `findings` to a temp `in.md`, run the compressor to `out.mq`, read it back.
async fn run_marqant(program: &str, prefix: &[String], findings: &str) -> Result<String, PpError> {
    use tokio::process::Command;

    let dir = tempfile::tempdir().map_err(|e| PpError::Marqant(format!("tempdir: {e}")))?;
    let in_path = dir.path().join("in.md");
    let out_path = dir.path().join("out.mq");
    tokio::fs::write(&in_path, findings)
        .await
        .map_err(|e| PpError::Marqant(format!("write in.md: {e}")))?;

    let status = Command::new(program)
        .args(prefix)
        .arg("compress")
        .arg(&in_path)
        .arg("-o")
        .arg(&out_path)
        .arg("--semantic")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| PpError::Marqant(format!("spawn {program}: {e}")))?;
    if !status.success() {
        return Err(PpError::Marqant(format!("mq exited with {status}")));
    }

    let bytes = read_capped(&out_path, MAX_MARQANT_OUTPUT).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn read_capped(path: &std::path::Path, max: u64) -> Result<Vec<u8>, PpError> {
    use tokio::io::AsyncReadExt;
    let f = tokio::fs::File::open(path)
        .await
        .map_err(|e| PpError::Marqant(format!("open out.mq: {e}")))?;
    let mut buf = Vec::new();
    f.take(max)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| PpError::Marqant(format!("read out.mq: {e}")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn stub_marqant_is_identity() {
        assert_eq!(
            StubMarqant.compress("brief body", Duration::from_millis(10)).await.unwrap(),
            "brief body"
        );
    }

    #[tokio::test]
    async fn subprocess_missing_binary_errors_to_fallback() {
        let mq = SubprocessMarqant::new("definitely-not-mq-xyz");
        let r = mq.compress("some findings", Duration::from_millis(500)).await;
        assert!(matches!(r, Err(PpError::Marqant(_))), "absent mq → Err → fallback");
    }

    #[tokio::test]
    async fn subprocess_empty_cmd_errors() {
        let mq = SubprocessMarqant::new("   ");
        assert!(mq.compress("x", Duration::from_millis(100)).await.is_err());
    }

    // Real subprocess round-trip through a fake `mq` that honours the exact
    // `compress <in> -o <out> --semantic` argv: it prepends "MQ:" to the input.
    // Proves file-arg I/O + argument positions without needing marqant installed.
    #[tokio::test]
    async fn subprocess_roundtrip_through_fake_mq() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fakemq.sh");
        // $1=compress $2=<in> $3=-o $4=<out> $5=--semantic
        std::fs::write(
            &script,
            "#!/bin/sh\ntest \"$1\" = compress || exit 3\ntest \"$5\" = --semantic || exit 4\nprintf 'MQ:' > \"$4\"\ncat \"$2\" >> \"$4\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mq = SubprocessMarqant::new(script.to_str().unwrap());
        let out = mq.compress("auth login flow", Duration::from_millis(2000)).await.unwrap();
        assert_eq!(out, "MQ:auth login flow", "compressor output flows back verbatim");
    }
}
