//! Memory-aware adk-rust callbacks: the `EntheaiAgent` counterpart to
//! `Agent::run_task_with_memory`'s pre-task retrieval/frozen-wake injection
//! and per-tool evidence recording.
//!
//! Scope note (see docs/superpowers/plans/2026-07-22-adk-rust-core-migration.md,
//! Task 5): before_model retrieval + frozen-node injection, and
//! after_tool_full evidence recording. The remaining post-task behavior
//! (`record_final_answer`/`ingest_transcript`) can't be replicated from a
//! callback at all — `adk_rust`'s `AfterAgentCallback` only receives
//! `Arc<dyn CallbackContext>`, which has no accessor for session/event
//! history (that lives on `InvocationContext`, one level up, never handed to
//! callbacks — confirmed against the vendored adk-core 1.0.0 source, no
//! downcast escape hatch exists either). That gap is closed instead in
//! `crate::event_bridge`, which drives `EntheaiAgent::run`'s `EventStream`
//! directly and so has natural, per-run local state to accumulate into
//! (Task 6).

use std::collections::HashSet;
use std::sync::Arc;

use adk_rust::{
    AdkError, AfterToolCallbackFull, BeforeModelCallback, BeforeModelResult, CallbackContext,
    Content, LlmRequest, Result as AdkResult,
};
use entheai_memory::{MemoryRuntime, MemoryScope, ToolEvidence};
use entheai_memory_pp::PromptProcessor;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;

use crate::AgentEvent;

/// Deadline for activating a frozen node (mirrors `crates/core/src/lib.rs`'s
/// `FROZEN_ACTIVATE_DEADLINE`).
const FROZEN_ACTIVATE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

fn user_text(content: &Content) -> String {
    content.parts.iter().filter_map(|p| p.text()).collect::<Vec<_>>().join(" ")
}

/// Injects pre-task retrieval context and frozen-node briefs, mirroring
/// `run_task_with_memory`'s pre-loop block. Fires at most once per session
/// (tracked via `injected_sessions`), since `before_model` runs once per model
/// call within a run but the injection is a once-per-task concern.
pub fn before_model_retrieval_callback(
    memory: Arc<MemoryRuntime>,
    pp: Option<Arc<PromptProcessor>>,
    injected_sessions: Arc<Mutex<HashSet<String>>>,
    event_tx: Option<UnboundedSender<AgentEvent>>,
) -> BeforeModelCallback {
    Box::new(move |ctx: Arc<dyn CallbackContext>, mut request: LlmRequest| {
        let memory = Arc::clone(&memory);
        let pp = pp.clone();
        let injected_sessions = Arc::clone(&injected_sessions);
        let event_tx = event_tx.clone();
        Box::pin(async move {
            {
                let mut seen = injected_sessions.lock().await;
                if !seen.insert(ctx.session_id().to_string()) {
                    return Ok(BeforeModelResult::Continue(request));
                }
            }

            // Retrieval injection (fresh `rposition` — mirrors run_task_with_memory
            // recomputing the user-message index per block, not reusing a stale one).
            if let Some(user_idx) = request.contents.iter().rposition(|c| c.role == "user") {
                let user_msg = user_text(&request.contents[user_idx]);
                if !user_msg.trim().is_empty() {
                    let retrieved = match &pp {
                        Some(p) => match p.retrieve(&user_msg).await {
                            Ok(Some(brief)) => Ok(Some(brief)),
                            Ok(None) | Err(_) => memory.retrieve_before(&user_msg).await,
                        },
                        None => memory.retrieve_before(&user_msg).await,
                    };
                    match retrieved {
                        Ok(Some(ctx_text)) => {
                            request
                                .contents
                                .insert(user_idx, Content::new("system").with_text(ctx_text));
                        }
                        Ok(None) => {}
                        Err(e) => {
                            if memory.config().strict {
                                return Err(AdkError::memory(e.to_string()));
                            }
                        }
                    }
                }
            }

            // Frozen-node wake injection — separate fresh `rposition` since the
            // retrieval insert above may have shifted the user message's index.
            if let Some(p) = &pp {
                if let Some(user_idx) = request.contents.iter().rposition(|c| c.role == "user") {
                    let user_msg = user_text(&request.contents[user_idx]);
                    for node in p.wake_frozen(&user_msg, 1) {
                        let brief = p.activate_frozen(&node, FROZEN_ACTIVATE_DEADLINE).await;
                        if let Some(tx) = &event_tx {
                            let preview: String = brief.chars().take(120).collect();
                            let _ = tx.send(AgentEvent::FrozenWoke {
                                name: node.name.clone(),
                                brief_preview: preview,
                            });
                        }
                        request
                            .contents
                            .insert(user_idx, Content::new("system").with_text(brief));
                    }
                }
            }

            Ok(BeforeModelResult::Continue(request))
        })
    })
}

/// Records tool evidence for spillover/trajectory, mirroring
/// `run_task_with_memory`'s per-tool-call block. `adk-rust`'s tool-calling
/// loop doesn't expose a stable per-call id to `AfterToolCallbackFull`, so
/// each evidence record gets a fresh id — this loses cross-retry dedup on
/// `record_tool_result`'s content-addressed key, an accepted, narrow gap
/// (retries aren't deduped; nothing else depends on id stability).
pub fn after_tool_evidence_callback(
    scope: MemoryScope,
    memory: Arc<MemoryRuntime>,
    pp: Option<Arc<PromptProcessor>>,
) -> AfterToolCallbackFull {
    Box::new(move |_ctx, tool, args, response| {
        let scope = scope.clone();
        let memory = Arc::clone(&memory);
        let pp = pp.clone();
        Box::pin(async move {
            let ev = ToolEvidence {
                call_id: uuid::Uuid::new_v4().to_string(),
                name: tool.name().to_string(),
                args: args.to_string(),
                result: response.to_string(),
                allowed: response.get("error").is_none(),
            };

            if let Some(p) = &pp {
                p.ingest_tool(&scope, &ev).await;
            }

            let result: AdkResult<Option<serde_json::Value>> =
                match memory.record_tool_result(&scope, &ev).await {
                    Ok(Some(pointer)) => Ok(Some(serde_json::json!(pointer))),
                    Ok(None) => Ok(None),
                    Err(e) => {
                        if memory.config().strict {
                            Err(AdkError::memory(e.to_string()))
                        } else {
                            Ok(None)
                        }
                    }
                };
            result
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_rust::{Artifacts, ReadonlyContext, ToolContext};
    use entheai_memory::{MemoryRuntimeConfig, SqliteStore};
    use serde_json::json;

    struct FakeCtx {
        content: Content,
    }

    #[async_trait::async_trait]
    impl ReadonlyContext for FakeCtx {
        fn invocation_id(&self) -> &str {
            "inv"
        }
        fn agent_name(&self) -> &str {
            "entheai"
        }
        fn user_id(&self) -> &str {
            "u"
        }
        fn app_name(&self) -> &str {
            "entheai"
        }
        fn session_id(&self) -> &str {
            "s1"
        }
        fn branch(&self) -> &str {
            ""
        }
        fn user_content(&self) -> &Content {
            &self.content
        }
    }

    #[async_trait::async_trait]
    impl CallbackContext for FakeCtx {
        fn artifacts(&self) -> Option<Arc<dyn Artifacts>> {
            None
        }
    }

    struct FakeTool;

    #[async_trait::async_trait]
    impl adk_rust::Tool for FakeTool {
        fn name(&self) -> &str {
            "read_file"
        }
        fn description(&self) -> &str {
            ""
        }
        fn declaration(&self) -> serde_json::Value {
            json!({})
        }
        async fn execute(
            &self,
            _ctx: Arc<dyn ToolContext>,
            _args: serde_json::Value,
        ) -> AdkResult<serde_json::Value> {
            Ok(json!({}))
        }
    }

    fn scope() -> MemoryScope {
        MemoryScope {
            session_id: "s1".into(),
            task_id: "t1".into(),
            cwd: std::env::temp_dir(),
            role: None,
        }
    }

    #[tokio::test]
    async fn after_tool_evidence_callback_overrides_response_with_spill_pointer() {
        let store = SqliteStore::open_memory(None).unwrap();
        let memory = Arc::new(MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig { enabled: true, tool_spill_chars: 5, ..Default::default() },
        ));
        let callback = after_tool_evidence_callback(scope(), Arc::clone(&memory), None);

        let ctx: Arc<dyn CallbackContext> =
            Arc::new(FakeCtx { content: Content::new("user").with_text("hi") });
        let tool: Arc<dyn adk_rust::Tool> = Arc::new(FakeTool);
        let response = json!("a very long tool output exceeding five chars easily");

        let overridden = callback(ctx, tool, json!({}), response)
            .await
            .expect("callback succeeds")
            .expect("large output must be spilled and overridden");
        assert!(
            overridden.as_str().unwrap().contains("memory://tools/"),
            "expected a spill pointer, got {overridden}"
        );
    }

    #[tokio::test]
    async fn after_tool_evidence_callback_keeps_small_output_unmodified() {
        let store = SqliteStore::open_memory(None).unwrap();
        let memory = Arc::new(MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig { enabled: true, tool_spill_chars: 10_000, ..Default::default() },
        ));
        let callback = after_tool_evidence_callback(scope(), Arc::clone(&memory), None);

        let ctx: Arc<dyn CallbackContext> =
            Arc::new(FakeCtx { content: Content::new("user").with_text("hi") });
        let tool: Arc<dyn adk_rust::Tool> = Arc::new(FakeTool);
        let response = json!("short");

        let result = callback(ctx, tool, json!({}), response).await.expect("callback succeeds");
        assert!(result.is_none(), "small output should not be overridden");
    }
}
