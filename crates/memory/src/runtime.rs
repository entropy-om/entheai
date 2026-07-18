//! Memory runtime — the hot-path integration between the agent loop and the
//! memory store. Handles pre-task retrieval, tool spillover, and post-task
//! trajectory/learning recording.
//!
//! All methods are best-effort by default (`strict = false`). Failures produce
//! log diagnostics but do not interrupt the task. Set `strict = true` to
//! convert memory errors into task failures.

use std::path::PathBuf;

use log::warn;

use crate::{MemoryError, Namespace, SharedMemory};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Controls memory runtime behaviour. All fields have sensible defaults.
#[derive(Debug, Clone)]
pub struct MemoryRuntimeConfig {
    /// Master switch. When `false`, every runtime method returns immediately.
    pub enabled: bool,
    /// When `true`, memory errors are fatal to the task.
    pub strict: bool,
    /// Max codebase results to inject.
    pub retrieve_codebase: usize,
    /// Max learnings results to inject.
    pub retrieve_learnings: usize,
    /// Max trajectory results to inject.
    pub retrieve_trajectories: usize,
    /// Hard cap on total memory-context characters injected into the prompt.
    pub max_context_chars: usize,
    /// Tool outputs longer than this are spilled to `tools` namespace.
    pub tool_spill_chars: usize,
    /// Tool names whose outputs are always treated as evidence.
    pub evidence_tools: Vec<String>,
}

impl Default for MemoryRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false, // off by default until stable
            strict: false,
            retrieve_codebase: 4,
            retrieve_learnings: 6,
            retrieve_trajectories: 3,
            max_context_chars: 12_000,
            tool_spill_chars: 8_000,
            evidence_tools: vec!["run_shell".into(), "search".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Binds a memory operation to a specific session and task.
#[derive(Debug, Clone)]
pub struct MemoryScope {
    pub session_id: String,
    pub task_id: String,
    pub cwd: PathBuf,
    pub role: Option<String>,
}

// ---------------------------------------------------------------------------
// Tool evidence
// ---------------------------------------------------------------------------

/// A record of a single tool call, captured for spillover and trajectory.
#[derive(Debug, Clone)]
pub struct ToolEvidence {
    pub call_id: String,
    pub name: String,
    pub args: String,
    pub result: String,
    pub allowed: bool,
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// The memory runtime. Wraps a [`SharedMemory`] and applies policy from
/// [`MemoryRuntimeConfig`].
pub struct MemoryRuntime {
    memory: SharedMemory,
    config: MemoryRuntimeConfig,
}

impl MemoryRuntime {
    /// Create a new runtime. If `config.enabled` is `false`, all methods
    /// become no-ops.
    pub fn new(memory: SharedMemory, config: MemoryRuntimeConfig) -> Self {
        Self { memory, config }
    }

    /// Borrow the runtime configuration (for strict-mode checks in callers).
    pub fn config(&self) -> &MemoryRuntimeConfig {
        &self.config
    }

    /// Build a system message to inject before the model call.
    ///
    /// Searches `codebase`, `learnings`, and `trajectories` using the latest
    /// user message as the query. Results are formatted with source labels
    /// and trimmed to `max_context_chars`.
    pub async fn retrieve_before(
        &self,
        latest_user_message: &str,
    ) -> Result<Option<String>, MemoryError> {
        if !self.config.enabled {
            return Ok(None);
        }
        if latest_user_message.trim().is_empty() {
            return Ok(None);
        }

        let mut blocks: Vec<String> = Vec::new();
        let mut chars_used = 0usize;

        // Namespace → (limit, label)
        let searches = [
            (
                Namespace::Codebase,
                self.config.retrieve_codebase,
                "codebase",
            ),
            (
                Namespace::Learnings,
                self.config.retrieve_learnings,
                "learnings",
            ),
            (
                Namespace::Trajectories,
                self.config.retrieve_trajectories,
                "trajectories",
            ),
        ];

        for (ns, limit, label) in searches {
            if limit == 0 {
                continue;
            }
            match self.memory.search(ns, latest_user_message, limit).await {
                Ok(results) => {
                    for se in results {
                        let line = format!(
                            "[{label} score={:.2} key={}]\n{}\n",
                            se.score, se.entry.key, se.entry.content,
                        );
                        if chars_used + line.len() > self.config.max_context_chars {
                            break;
                        }
                        chars_used += line.len();
                        blocks.push(line);
                    }
                }
                Err(e) => {
                    warn!("memory retrieve_before error (non-strict, continuing): {e}");
                    if self.config.strict {
                        return Err(e);
                    }
                }
            }
        }

        if blocks.is_empty() {
            return Ok(None);
        }

        let header = "Memory context:\n\n";
        let body = blocks.join("\n");
        Ok(Some(format!("{header}{body}")))
    }

    /// Record a tool call result. If the output is large or evidence-bearing,
    /// it is spilled to `tools` namespace and the caller receives a compact
    /// pointer + preview.
    pub async fn record_tool_result(
        &self,
        scope: &MemoryScope,
        evidence: &ToolEvidence,
    ) -> Result<Option<String>, MemoryError> {
        if !self.config.enabled {
            return Ok(None);
        }

        let should_spill = evidence.result.len() > self.config.tool_spill_chars
            || evidence.result.starts_with("error:")
            || self.config.evidence_tools.contains(&evidence.name);

        if !should_spill {
            return Ok(None);
        }

        let key = tool_key(scope, &evidence.call_id);
        // Char-aware truncation: `evidence.result` is arbitrary tool output, so a
        // raw byte slice (`&result[..500]`) panics when byte 500 lands mid multi-
        // byte UTF-8 char (any non-ASCII output). Reuse the safe truncator.
        let preview = truncate_str(&evidence.result, 500);

        if let Err(e) = self
            .memory
            .store(
                Namespace::Tools,
                &key,
                &evidence.result,
                Some(serde_json::json!({
                    "tool": evidence.name,
                    "args": evidence.args,
                    "allowed": evidence.allowed,
                    "call_id": evidence.call_id,
                })),
            )
            .await
        {
            warn!("memory record_tool_result store error (non-strict, continuing): {e}");
            if self.config.strict {
                return Err(e);
            }
            return Ok(None);
        }

        Ok(Some(format!(
            "tool result stored in memory://tools/{key}\npreview:\n{preview}",
        )))
    }

    /// Record a completed task. Writes a structured trajectory entry and
    /// extracts durable learnings candidates.
    pub async fn record_final_answer(
        &self,
        scope: &MemoryScope,
        model: &str,
        answer_preview: &str,
        tool_evidence: &[ToolEvidence],
    ) -> Result<(), MemoryError> {
        if !self.config.enabled {
            return Ok(());
        }

        let now = timestamp_ms();

        // Trajectory entry.
        let traj_key = traj_key(scope);
        let traj = serde_json::json!({
            "schema": "entheai.trajectory.v1",
            "session_id": scope.session_id,
            "task_id": scope.task_id,
            "cwd": scope.cwd.to_string_lossy(),
            "role": scope.role,
            "finished_at": now,
            "model": model,
            "tool_calls": tool_evidence.iter().map(|t| serde_json::json!({
                "name": t.name,
                "allowed": t.allowed,
                "args": truncate_str(&t.args, 200),
            })).collect::<Vec<_>>(),
            "outcome": "answered",
            "final_answer_preview": truncate_str(answer_preview, 500),
        });

        if let Err(e) = self
            .memory
            .store(Namespace::Trajectories, &traj_key, &traj.to_string(), None)
            .await
        {
            warn!("memory record_final_answer store error (non-strict, continuing): {e}");
            if self.config.strict {
                return Err(e);
            }
        }

        // Deterministic learnings extraction.
        self.extract_learnings(scope, tool_evidence).await?;

        Ok(())
    }

    /// Deterministic extraction of durable learnings from this task.
    async fn extract_learnings(
        &self,
        scope: &MemoryScope,
        evidence: &[ToolEvidence],
    ) -> Result<(), MemoryError> {
        for (idx, ev) in evidence.iter().enumerate() {
            let outcome = if ev.allowed && !ev.result.starts_with("error:") {
                "succeeded"
            } else if !ev.allowed {
                "denied"
            } else {
                "failed"
            };

            let learning = serde_json::json!({
                "schema": "entheai.learning.v1",
                "source": "post_task_extraction",
                "session_id": scope.session_id,
                "task_id": scope.task_id,
                "cwd": scope.cwd.to_string_lossy(),
                "confidence": 0.5,
                "tags": ["tool", outcome, &ev.name],
            });

            let key = format!("{}/{}/tool/{idx}", scope.session_id, scope.task_id);
            let content = format!(
                "tool `{}` {} with args `{}` → {}",
                ev.name,
                outcome,
                truncate_str(&ev.args, 200),
                truncate_str(&ev.result, 300),
            );

            if let Err(e) = self
                .memory
                .store(Namespace::Learnings, &key, &content, Some(learning))
                .await
            {
                warn!("memory extract_learnings store error (non-strict, continuing): {e}");
                if self.config.strict {
                    return Err(e);
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Key builders
// ---------------------------------------------------------------------------

fn tool_key(scope: &MemoryScope, call_id: &str) -> String {
    format!("{}/{}/{}", scope.session_id, scope.task_id, call_id)
}

fn traj_key(scope: &MemoryScope) -> String {
    format!("{}/{}", scope.session_id, scope.task_id)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;
    use std::sync::Arc;

    fn test_scope() -> MemoryScope {
        MemoryScope {
            session_id: "s1".into(),
            task_id: "t1".into(),
            cwd: PathBuf::from("/tmp/test"),
            role: None,
        }
    }

    #[tokio::test]
    async fn retrieve_before_disabled_returns_none() {
        let store = SqliteStore::open_memory(None).unwrap();
        let rt = MemoryRuntime::new(Arc::new(store), MemoryRuntimeConfig::default());
        let result = rt.retrieve_before("hello").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn retrieve_before_empty_input_returns_none() {
        let store = SqliteStore::open_memory(None).unwrap();
        let rt = MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig {
                enabled: true,
                ..Default::default()
            },
        );
        let result = rt.retrieve_before("   ").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn record_tool_result_small_output_not_spilled() {
        let store = SqliteStore::open_memory(None).unwrap();
        let rt = MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig {
                enabled: true,
                tool_spill_chars: 1000,
                ..Default::default()
            },
        );
        let ev = ToolEvidence {
            call_id: "c1".into(),
            name: "read_file".into(),
            args: "{}".into(),
            result: "small output".into(),
            allowed: true,
        };
        let spill = rt.record_tool_result(&test_scope(), &ev).await.unwrap();
        assert!(spill.is_none());
    }

    #[tokio::test]
    async fn record_tool_result_large_output_spilled() {
        let store = SqliteStore::open_memory(None).unwrap();
        let rt = MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig {
                enabled: true,
                tool_spill_chars: 10,
                ..Default::default()
            },
        );
        let ev = ToolEvidence {
            call_id: "c1".into(),
            name: "run_shell".into(),
            args: "{}".into(),
            result: "a".repeat(200),
            allowed: true,
        };
        let spill = rt.record_tool_result(&test_scope(), &ev).await.unwrap();
        assert!(spill.is_some());
        let text = spill.unwrap();
        assert!(text.contains("memory://tools/"));
        assert!(text.contains("preview:"));
    }

    #[tokio::test]
    async fn record_tool_result_evidence_tool_always_spilled() {
        let store = SqliteStore::open_memory(None).unwrap();
        let rt = MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig {
                enabled: true,
                tool_spill_chars: 10_000,
                evidence_tools: vec!["run_shell".into()],
                ..Default::default()
            },
        );
        let ev = ToolEvidence {
            call_id: "c1".into(),
            name: "run_shell".into(),
            args: "{}".into(),
            result: "ok".into(),
            allowed: true,
        };
        let spill = rt.record_tool_result(&test_scope(), &ev).await.unwrap();
        assert!(spill.is_some());
    }

    #[tokio::test]
    async fn record_final_answer_writes_trajectory() {
        let store = SqliteStore::open_memory(None).unwrap();
        let rt = MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig {
                enabled: true,
                ..Default::default()
            },
        );
        rt.record_final_answer(
            &test_scope(),
            "zen/deepseek-v4-pro",
            "final answer text",
            &[ToolEvidence {
                call_id: "c1".into(),
                name: "read_file".into(),
                args: "{}".into(),
                result: "content".into(),
                allowed: true,
            }],
        )
        .await
        .unwrap();
    }
}
