//! `CoderExecutor` impl: run each fan-out coder sub-task via the Antigravity CLI
//! (`agy`) inside its isolated worktree, leaving UNCOMMITTED changes for
//! `run_fanout`'s commit/verify/integrate path — entheai orchestrates + integrates
//! while agy (Google's Ultra models) does the coding.
//!
//! This is the "recursive development" path (entheai developing entheai), so it
//! carries two safeguards the naive version lacks:
//!   * a DEPTH GUARD — each spawn increments `ENTHEAI_FANOUT_DEPTH`; past `MAX_DEPTH`
//!     the executor declines (`workers_available()` → false → local fallback), so an
//!     entheai coder that itself invokes entheai can't recurse without bound;
//!   * a LAYER-AWARE PREAMBLE — every coder prompt is prefixed with its layer + role
//!     and an explicit "stay in your layer, don't start another fan-out", so context
//!     and position are unambiguous at every layer.
//!
//! Any failure (agy missing, error, no change) returns `None` → local fallback.

use std::path::Path;
use std::sync::Arc;

/// Env var threading the current fan-out depth to nested entheai/agy processes.
const DEPTH_ENV: &str = "ENTHEAI_FANOUT_DEPTH";
/// Hard cap on recursive fan-out depth (entheai-develops-entheai safety).
const MAX_DEPTH: u32 = 3;

pub struct AgyExecutor {
    model: String,
    depth: u32,
}

impl AgyExecutor {
    /// Build an executor. Reads the current depth from `ENTHEAI_FANOUT_DEPTH`
    /// (0 at the top level); each `execute` spawns agy at `depth + 1`.
    pub fn new(model: impl Into<String>) -> Arc<Self> {
        let depth = std::env::var(DEPTH_ENV)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Arc::new(Self { model: model.into(), depth })
    }
}

/// Layer-aware coder prompt: a system-style preamble (layer + role + guardrails)
/// followed by the focused task. Pure — unit-tested.
fn coder_prompt(role: &str, task: &str, depth: u32) -> String {
    format!(
        "[entheai fan-out · layer {depth} · role: {role}]\n\
         You are ONE parallel coder in an entheai fan-out, working in an isolated git \
         worktree of this project. Make ONLY the change described below by editing files in \
         the current directory. Do NOT commit — leave your edits as working-tree changes. Do \
         NOT start another fan-out, orchestration, or long-running agent; stay in your layer. \
         Be surgical: touch only what the task needs.\n\n\
         Task ({role}):\n{task}"
    )
}

async fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn git_ok(dir: &Path, args: &[&str]) -> bool {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl crate::CoderExecutor for AgyExecutor {
    async fn workers_available(&self) -> bool {
        if self.depth >= MAX_DEPTH {
            log::warn!(
                "agy executor: fan-out depth {} ≥ max {MAX_DEPTH} — coders run locally (recursion guard)",
                self.depth
            );
            return false;
        }
        tokio::process::Command::new("agy")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    async fn execute(
        &self,
        _session: &str,
        _index: usize,
        base_sha: &str,
        worktree_path: &Path,
        role: &str,
        task: &str,
    ) -> Option<String> {
        let prompt = coder_prompt(role, task, self.depth);
        // Run agy in the coder's worktree. `--sandbox` restricts terminal side
        // effects; `--dangerously-skip-permissions` lets it edit files
        // non-interactively (bounded by the sandbox + the worktree). The depth env
        // is incremented so any nested entheai/agy sees the deeper layer.
        let out = tokio::process::Command::new("agy")
            .arg("-p")
            .arg(&prompt)
            .arg("--model")
            .arg(&self.model)
            .arg("--sandbox")
            .arg("--dangerously-skip-permissions")
            .arg("--print-timeout")
            .arg("10m")
            .current_dir(worktree_path)
            .env(DEPTH_ENV, (self.depth + 1).to_string())
            .output()
            .await
            .ok()?;
        if !out.status.success() {
            log::warn!("agy coder (layer {}, {role}) failed → local fallback", self.depth);
            return None;
        }
        let log = String::from_utf8_lossy(&out.stdout).to_string();

        // The contract wants UNCOMMITTED changes. If agy committed (HEAD moved off
        // the base), soft-reset so its edits return to the working tree.
        if let Some(head) = git_stdout(worktree_path, &["rev-parse", "HEAD"]).await {
            if head != base_sha {
                let _ = git_ok(worktree_path, &["reset", "--soft", base_sha]).await;
                let _ = git_ok(worktree_path, &["reset"]).await; // unstage → uniform working tree
            }
        }
        // No working-tree change → treat as a no-op (local fallback).
        let dirty = git_stdout(worktree_path, &["status", "--porcelain"])
            .await
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if !dirty {
            return None;
        }
        Some(log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coder_prompt_carries_layer_role_task_and_guardrails() {
        let p = coder_prompt("reviewer", "fix the bug in foo.rs", 2);
        assert!(p.contains("layer 2"));
        assert!(p.contains("role: reviewer"));
        assert!(p.contains("fix the bug in foo.rs"));
        assert!(p.to_lowercase().contains("do not commit"));
        assert!(p.to_lowercase().contains("stay in your layer"));
    }

    #[test]
    fn max_depth_is_a_sane_cap() {
        assert!((1..=5).contains(&MAX_DEPTH), "recursion cap must be bounded + small");
    }
}
