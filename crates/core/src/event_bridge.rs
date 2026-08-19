//! Drives `EntheaiAgent::run_with_history`'s `adk_rust::EventStream` and translates it into
//! the TUI/CLI-facing `AgentEvent` vocabulary, closing the post-task memory
//! gap `crate::memory_callbacks` left open (see its module doc).
//!
//! adk-rust's `Event` stream only reflects what came *back* from the model —
//! there's no discrete "about to call the model" signal (unlike the old
//! `Agent::stream_turn`'s explicit `AgentEvent::Thinking` emission), and the
//! before_model-injected retrieval/frozen-node briefs are transparent to the
//! caller by design, so they never appear as stream content either (frozen
//! wake is instead signalled directly by `memory_callbacks` via its own
//! `event_tx`, wired in by the caller). This function approximates
//! "thinking" locally: once at the start of a run, and again after every
//! tool result (since the model is about to be called again).
//!
//! `record_final_answer`/`ingest_transcript` need state accumulated across
//! the whole run (tool evidence, the reconstructed transcript). Because this
//! function owns that state as plain local variables scoped to one run — not
//! a `Fn` closure shared across every call the agent ever makes — no
//! session-keyed shared state is needed to close that gap; it falls out of
//! driving the stream directly instead of doing it from inside a callback.

use std::collections::HashMap;
use std::sync::Arc;

use adk_rust::Part;
use entheai_memory::{MemoryRuntime, MemoryScope, ToolEvidence};
use entheai_memory_pp::PromptProcessor;
use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use crate::entheai_agent::EntheaiAgent;
use crate::{truncate_preview, AgentEvent};

/// True when `result` matches `AdkToolAdapter`'s denial/error shape: a bare
/// `{"error": ...}` object (an object whose only key is `error`). Tool data
/// that merely *contains* an `error` field alongside other keys (e.g.
/// `{"error":"x","rows":[...]}`) is legitimate output and must not be
/// recorded as a denial.
fn is_denial_shape(result: &serde_json::Value) -> bool {
    matches!(
        result,
        serde_json::Value::Object(map) if map.len() == 1 && map.contains_key("error")
    )
}

/// Runs `agent` against `user_message`, forwarding live progress as
/// `AgentEvent`s on `event_tx` and — when `memory`/`pp` are given — recording
/// the final answer's trajectory and raw transcript once the run completes.
/// Returns the final answer text.
#[allow(clippy::too_many_arguments)]
pub async fn run_with_events(
    agent: &EntheaiAgent,
    prior_turns: &[(Arc<str>, Arc<str>)],
    user_message: &str,
    model: &str,
    event_tx: UnboundedSender<AgentEvent>,
    memory: Option<Arc<MemoryRuntime>>,
    pp: Option<Arc<PromptProcessor>>,
    scope: MemoryScope,
) -> anyhow::Result<String> {
    let _ = event_tx.send(AgentEvent::Thinking);

    let (session_id, mut stream) = agent.run_with_history(prior_turns, user_message).await?;
    let mut answer = String::new();
    // Accumulates partial (streamed-delta) text for the current turn. Some
    // OpenAI-compatible providers — notably Ollama-backed ones (`fp_ollama`,
    // which is what coder.vaked.dev's free tier serves) — stream the answer
    // only as partial deltas and terminate with a `{"choices":[]}` + usage
    // chunk, emitting NO final non-partial text event. So the `answer = text`
    // path below never fires and the reply is silently dropped. We fall back to
    // this after the loop. Reset at every turn boundary (a final text answer,
    // or any tool call/result) so it only ever holds the LAST turn's stream.
    let mut streamed_text = String::new();
    // Seed the transcript with the full conversation (prior turns + the
    // current message) so memory records the whole exchange, not just the
    // latest message. `prior_turns` uses the same `(role, text)` shape the
    // transcript already carries (`"user"`/`"assistant"`).
    let mut transcript: Vec<(Arc<str>, Arc<str>)> = prior_turns.to_vec();
    transcript.push(("user".into(), Arc::from(user_message.to_string())));
    let mut tool_evidence: Vec<ToolEvidence> = Vec::new();
    // FunctionCall.id -> (name, args), consumed by the matching FunctionResponse
    // so ToolEvidence carries the args the call was actually made with.
    let mut pending_calls: HashMap<String, (String, String)> = HashMap::new();
    // First mid-stream error, applied after the session is released below.
    let mut stream_err: Option<anyhow::Error> = None;
    // Per-event-gap idle timeout on the PROVIDER (2× the configured value).
    // Not applied while a tool call is in flight: adk runs the tool inside the
    // stream between the FunctionCall and FunctionResponse events, and that
    // window holds the permission prompt (the user may sit at the y/N modal for
    // minutes) plus the tool's own bound (`[tools].shell_timeout_secs`, the MCP
    // call timeout) — none of which is provider idleness.
    let idle = agent.request_timeout().saturating_mul(2);
    let mut awaiting_tool = false;

    loop {
        let next = if awaiting_tool {
            stream.next().await
        } else {
            match tokio::time::timeout(idle, stream.next()).await {
                Ok(next) => next,
                Err(_) => {
                    stream_err = Some(anyhow::anyhow!(
                        "provider stream idle timeout after {idle:?}"
                    ));
                    break;
                }
            }
        };
        let Some(ev) = next else { break };
        let ev = match ev {
            Ok(ev) => ev,
            Err(e) => {
                stream_err = Some(e.into());
                break;
            }
        };
        let Some(content) = ev.content() else {
            continue;
        };

        if ev.llm_response.partial {
            for part in &content.parts {
                if let Some(t) = part.text() {
                    streamed_text.push_str(t);
                    let _ = event_tx.send(AgentEvent::Token(t.into()));
                }
            }
            continue;
        }

        let text: String = content.parts.iter().filter_map(|p| p.text()).collect();
        let has_calls = content
            .parts
            .iter()
            .any(|p| matches!(p, Part::FunctionCall { .. }));
        let has_results = content
            .parts
            .iter()
            .any(|p| matches!(p, Part::FunctionResponse { .. }));
        if has_calls {
            awaiting_tool = true;
        }
        if has_results {
            awaiting_tool = false;
        }

        // A non-partial, pure-text turn (no calls, no results) is a candidate
        // final answer — overwritten by any later such turn, matching
        // `EntheaiAgent::run_to_text`'s "last one wins" contract.
        if !text.is_empty() && !has_calls && !has_results {
            answer = text.clone();
            streamed_text.clear(); // a real final answer arrived — drop partials
            transcript.push(("assistant".into(), Arc::from(text)));
        } else if has_calls || has_results {
            // Turn boundary: partials streamed before this were thinking-text
            // ahead of a tool call, or a tool round — not the final answer.
            // Also clear a prior non-partial `answer`: any later call/result
            // turn proves that earlier text was preamble, not the real reply,
            // so the partial-only fallback below must not return it.
            streamed_text.clear();
            answer.clear();
        }

        for part in &content.parts {
            match part {
                Part::FunctionCall { name, args, id, .. } => {
                    let args_str = args.to_string();
                    if let Some(id) = id {
                        pending_calls.insert(id.clone(), (name.clone(), args_str.clone()));
                    }
                    let _ = event_tx.send(AgentEvent::ToolStarted {
                        name: name.clone(),
                        args: args_str,
                    });
                }
                Part::FunctionResponse {
                    function_response,
                    id,
                } => {
                    let result_str = function_response.response.to_string();
                    let args_str = id
                        .as_ref()
                        .and_then(|i| pending_calls.remove(i))
                        .map(|(_, args)| args)
                        .unwrap_or_default();
                    let call_id = id
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    let allowed = !is_denial_shape(&function_response.response);

                    tool_evidence.push(ToolEvidence {
                        call_id,
                        name: function_response.name.clone(),
                        args: args_str,
                        result: result_str.clone(),
                        allowed,
                    });
                    transcript.push(("function".into(), Arc::from(result_str.clone())));
                    let _ = event_tx.send(AgentEvent::ToolFinished {
                        name: function_response.name.clone(),
                        result: result_str,
                    });
                    // The model is about to be called again with this result.
                    let _ = event_tx.send(AgentEvent::Thinking);
                }
                _ => {}
            }
        }
    }

    // Fallback for providers that stream the answer only as partial deltas and
    // never emit a final non-partial text event (see `streamed_text` above).
    // Without this, one-shot — and the default free-tier experience — is
    // silently empty even though the model replied in full.
    if answer.is_empty() && !streamed_text.is_empty() {
        answer = streamed_text.clone();
        transcript.push(("assistant".into(), Arc::from(streamed_text)));
    }

    // The run's stream is fully consumed (or failed) — release its per-run
    // session so the session service and the `injected_sessions` marker don't
    // grow without bound across runs on a long-lived agent.
    agent.cleanup_run(&session_id).await;
    if let Some(e) = stream_err {
        return Err(e);
    }

    if let Some(mem) = &memory {
        let preview = truncate_preview(&answer, 500);
        if let Err(e) = mem
            .record_final_answer(&scope, model, &preview, &tool_evidence)
            .await
        {
            log::warn!("event_bridge record_final_answer failed (continuing): {e}");
        }
    }
    if let Some(p) = &pp {
        // `ingest_transcript` returns `()` — its raw-store failure is logged
        // inside `PromptProcessor` itself (`log::warn!` "pp ingest_transcript
        // failed (continuing)"), the same continue-on-error contract as the
        // `record_final_answer` branch above; nothing to handle here.
        p.ingest_transcript(&scope, &transcript, &answer).await;
    }

    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use entheai_config::ProviderConfig;
    use entheai_permission::{Grant, Policy, Prompter};
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct AllowAll;
    #[async_trait::async_trait]
    impl Prompter for AllowAll {
        async fn confirm(&mut self, _tool: &str, _args: &str) -> Grant {
            Grant::Allow
        }
    }

    struct EchoTool;
    #[async_trait::async_trait]
    impl entheai_tools::Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "echo",
                    "parameters": {
                        "type": "object",
                        "properties": { "text": { "type": "string" } },
                        "required": ["text"]
                    }
                }
            })
        }
        async fn call(&self, args: serde_json::Value) -> Result<String, entheai_tools::ToolError> {
            Ok(format!("echoed: {}", args["text"].as_str().unwrap_or("")))
        }
    }

    async fn mock_tool_call_then_final_answer(server: &MockServer, answer: &str) {
        let tool_call_body = "data: {\"id\":\"t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"echo\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n\
             data: {\"id\":\"t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"text\\\":\\\"hi\\\"}\"}}]},\"finish_reason\":null}]}\n\n\
             data: {\"id\":\"t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
             data: [DONE]\n\n";
        let final_body = format!(
            "data: {{\"id\":\"t\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{answer}\"}},\"finish_reason\":\"stop\"}}]}}\n\n\
             data: [DONE]\n\n"
        );

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(tool_call_body)
                    .insert_header("Content-Type", "text/event-stream"),
            )
            .up_to_n_times(1)
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(final_body)
                    .insert_header("Content-Type", "text/event-stream"),
            )
            .mount(server)
            .await;
    }

    fn scope() -> MemoryScope {
        MemoryScope {
            session_id: "s1".into(),
            task_id: "t1".into(),
            cwd: std::env::temp_dir(),
            role: None,
        }
    }

    async fn build_agent(server: &MockServer) -> EntheaiAgent {
        let mut providers = HashMap::new();
        providers.insert(
            "test".to_string(),
            ProviderConfig {
                base_url: server.uri(),
                api_key_env: None,
                ..Default::default()
            },
        );
        let mut registry = entheai_tools::ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        EntheaiAgent::new(
            "test/model",
            &providers,
            &registry,
            Arc::new(Policy::new(true, vec![])),
            Arc::new(tokio::sync::Mutex::new(AllowAll)),
            25,
        )
        .expect("agent builds")
    }

    // Regression: a tool that legitimately returns error-shaped *data* (an
    // `error` field alongside other keys) used to be recorded as denied,
    // because any top-level `"error"` key flipped `allowed` to false. Only the
    // adapter's actual denial shape — a bare `{"error": ...}` object — means
    // denied.
    #[test]
    fn is_denial_shape_distinguishes_bare_error_from_data_containing_error() {
        // Adapter denial/error shape: a bare error object.
        assert!(is_denial_shape(&serde_json::json!({"error": "denied"})));
        assert!(is_denial_shape(
            &serde_json::json!({"error": "permission denied"})
        ));
        // Legitimate data that merely *contains* an `error` field.
        assert!(!is_denial_shape(
            &serde_json::json!({"error": "x", "rows": [1, 2, 3]})
        ));
        // Non-object results can never be denials.
        assert!(!is_denial_shape(&serde_json::json!("plain text")));
        assert!(!is_denial_shape(&serde_json::json!([{"error": "x"}])));
    }

    #[tokio::test]
    async fn forwards_tokens_and_tool_lifecycle_and_returns_final_answer() {
        let server = MockServer::start().await;
        mock_tool_call_then_final_answer(&server, "final answer").await;
        let agent = build_agent(&server).await;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let answer = run_with_events(
            &agent,
            &[],
            "please echo hi",
            "test/model",
            tx,
            None,
            None,
            scope(),
        )
        .await
        .expect("run succeeds");
        assert_eq!(answer, "final answer");

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolStarted { name, .. } if name == "echo")),
            "expected a ToolStarted(echo) event, got {events:?}"
        );
        assert!(
            events.iter().any(
                |e| matches!(e, AgentEvent::ToolFinished { name, result } if name == "echo" && result.contains("echoed: hi"))
            ),
            "expected a ToolFinished(echo) event carrying the tool's result, got {events:?}"
        );
    }

    // Regression: Ollama-backed OpenAI-compat streams (what coder.vaked.dev's
    // free tier serves) send the answer only as partial deltas and end with an
    // empty-content `finish_reason:"stop"` chunk + a `{"choices":[]}` usage
    // chunk — no final non-partial text event. The reply must still come
    // through; before the `streamed_text` fallback it was silently empty, which
    // broke the default free-tier one-shot introduced in v4.2.0.
    #[tokio::test]
    async fn partial_only_ollama_style_stream_still_returns_the_answer() {
        let server = MockServer::start().await;
        let body = "data: {\"id\":\"o\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n\
             data: {\"id\":\"o\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\", free\"},\"finish_reason\":null}]}\n\n\
             data: {\"id\":\"o\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" tier\"},\"finish_reason\":null}]}\n\n\
             data: {\"id\":\"o\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\"}]}\n\n\
             data: {\"id\":\"o\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":3,\"total_tokens\":8}}\n\n\
             data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .insert_header("Content-Type", "text/event-stream"),
            )
            .mount(&server)
            .await;
        let agent = build_agent(&server).await;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let answer = run_with_events(&agent, &[], "say hi", "test/model", tx, None, None, scope())
            .await
            .expect("run succeeds");
        assert_eq!(
            answer, "Hello, free tier",
            "partial-only (Ollama-style) stream must still yield the reply"
        );

        // The tokens were still forwarded live to the UI channel as they streamed.
        let mut toks = String::new();
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::Token(t) = ev {
                toks.push_str(&t);
            }
        }
        assert_eq!(toks, "Hello, free tier", "tokens also stream to the UI");
    }

    #[tokio::test]
    async fn records_final_answer_and_transcript_when_memory_present() {
        use entheai_memory::{MemoryRuntime, MemoryRuntimeConfig, Namespace, SqliteStore};

        let server = MockServer::start().await;
        mock_tool_call_then_final_answer(&server, "final answer").await;
        let agent = build_agent(&server).await;

        let store: Arc<dyn entheai_memory::Memory> =
            Arc::new(SqliteStore::open_memory(None).unwrap());
        let memory = Arc::new(MemoryRuntime::new(
            Arc::clone(&store),
            MemoryRuntimeConfig {
                enabled: true,
                ..Default::default()
            },
        ));

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let answer = run_with_events(
            &agent,
            &[],
            "please echo hi",
            "test/model",
            tx,
            Some(Arc::clone(&memory)),
            None,
            scope(),
        )
        .await
        .expect("run succeeds");
        assert_eq!(answer, "final answer");

        let trajectories = store.list(Namespace::Trajectories, 10, 0).await.unwrap();
        assert_eq!(trajectories.len(), 1, "expected one recorded trajectory");
        assert!(trajectories[0].content.contains("final answer"));
    }

    #[tokio::test]
    async fn prior_turns_are_seeded_into_the_outbound_request() {
        use wiremock::matchers::body_string_contains;

        let server = MockServer::start().await;
        let body = "data: {\"id\":\"t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("EARLIER_MARKER_TEXT"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .insert_header("Content-Type", "text/event-stream"),
            )
            .mount(&server)
            .await;
        let agent = build_agent(&server).await;

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let prior: Vec<(Arc<str>, Arc<str>)> = vec![
            (
                "user".into(),
                "EARLIER_MARKER_TEXT from a prior turn".into(),
            ),
            ("assistant".into(), "acknowledged".into()),
        ];
        // Fails with a wiremock "no matching mock" error unless the seeded
        // prior turn's marker text reached this outbound request.
        let answer = run_with_events(
            &agent,
            &prior,
            "what did I say earlier?",
            "test/model",
            tx,
            None,
            None,
            scope(),
        )
        .await
        .expect("run succeeds, proving prior_turns reached the outbound request");
        assert_eq!(answer, "ok");
    }
}
