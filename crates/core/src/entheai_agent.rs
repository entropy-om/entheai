//! `EntheaiAgent` — the adk-rust-backed replacement for `Agent<P>::run_task`.
//! See docs/superpowers/plans/2026-07-22-adk-rust-core-migration.md, Task 4.
//!
//! `adk_runner::Runner::session_service` is a private field with no public
//! accessor (confirmed against the vendored adk-runner 1.0.0 source), so this
//! wrapper holds its own `Arc<dyn SessionService>` alongside the `Runner`
//! rather than trying to recover it from the runner after construction.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use adk_rust::agent::LlmAgentBuilder;
use adk_rust::runner::Runner;
use adk_rust::session::{CreateRequest, DeleteRequest, InMemorySessionService, SessionService};
use adk_rust::Content;
use entheai_config::ProviderConfig;
use entheai_permission::{Policy, Prompter};

/// `(memory, prompt-processor, scope, brain-event sink)` — the inputs
/// `new_with_memory` threads into `build`'s memory-aware callback wiring.
type MemoryCtx = (
    Arc<entheai_memory::MemoryRuntime>,
    Option<Arc<entheai_memory_pp::PromptProcessor>>,
    entheai_memory::MemoryScope,
    Option<tokio::sync::mpsc::UnboundedSender<crate::AgentEvent>>,
);

/// Clamps `[inference].max_tokens` to `i32::MAX` before the cast, so an
/// oversized config value doesn't wrap negative in `max_output_tokens`
/// (which takes an `i32`).
fn clamp_max_tokens(max_tokens: u32) -> i32 {
    max_tokens.min(i32::MAX as u32) as i32
}

pub struct EntheaiAgent {
    runner: Runner,
    sessions: Arc<dyn SessionService>,
    app_name: String,
    /// Per-run session markers used by the memory before-model callback
    /// (`Some` only for memory-aware agents); pruned in [`Self::cleanup_run`]
    /// so the set doesn't grow one uuid per run on long-lived agent reuse.
    injected_sessions: Option<Arc<tokio::sync::Mutex<HashSet<String>>>>,
    /// Per-event-gap idle timeout applied while consuming a run's stream.
    /// Derived from `[inference].request_timeout_secs` (default 120s). A
    /// stalled provider yields an error instead of hanging the caller forever.
    request_timeout: std::time::Duration,
}

impl EntheaiAgent {
    /// The per-event idle timeout used when consuming a run's stream. Stream
    /// consumption applies `2×` this value as the gap between events (a tool
    /// call emits nothing for up to `run_shell`'s 120s cap, so the bare value
    /// is too tight; see `run_to_text` / `event_bridge::run_with_events`).
    pub fn request_timeout(&self) -> std::time::Duration {
        self.request_timeout
    }
}

impl EntheaiAgent {
    pub fn new(
        model_spec: &str,
        providers: &HashMap<String, ProviderConfig>,
        registry: &entheai_tools::ToolRegistry,
        policy: Arc<Policy>,
        prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
        max_iterations: u32,
    ) -> anyhow::Result<Self> {
        Self::new_with_instruction(
            model_spec,
            None,
            &entheai_config::InferenceConfig::default(),
            providers,
            registry,
            policy,
            prompter,
            max_iterations,
        )
    }

    /// Like [`Self::new`], with a system `instruction` and `[inference]`
    /// settings applied — for callers with no memory but a per-agent system
    /// prompt (e.g. the fan-out orchestrator's several differently-prompted
    /// agents).
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_instruction(
        model_spec: &str,
        instruction: Option<&str>,
        inference: &entheai_config::InferenceConfig,
        providers: &HashMap<String, ProviderConfig>,
        registry: &entheai_tools::ToolRegistry,
        policy: Arc<Policy>,
        prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
        max_iterations: u32,
    ) -> anyhow::Result<Self> {
        Self::build(
            model_spec,
            instruction,
            inference,
            providers,
            registry,
            policy,
            prompter,
            max_iterations,
            None,
        )
    }

    /// Memory-aware constructor: wires pre-task retrieval/frozen-node
    /// injection (`before_model`) and per-tool evidence recording
    /// (`after_tool_full`), mirroring `Agent::run_task_with_memory`'s
    /// memory-enabled path. `event_tx`, if given, receives an
    /// `AgentEvent::FrozenWoke` whenever the before_model callback injects a
    /// frozen-node brief — the event stream itself never surfaces this since
    /// the injection is transparent to the caller by design (same as the
    /// retrieval brief). See `crate::memory_callbacks` and
    /// `crate::event_bridge` for what is and isn't covered.
    ///
    /// `inference.request_timeout_secs` has no adk-rust 1.0.0 `OpenAIClient`
    /// equivalent (confirmed: it hardcodes `reqwest::Client::new()` with no
    /// timeout surface), so it is NOT applied at the client layer. It IS
    /// applied at stream consumption as a per-event-gap idle timeout (see
    /// [`Self::request_timeout`]) so a stalled provider can't hang the caller
    /// forever. `.retries` is likewise not applied — a retried run would
    /// re-execute tools (side effects), so it stays intentionally inert.
    /// `temperature`/`max_tokens` carry over via
    /// `LlmAgentBuilder::temperature`/`max_output_tokens`.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_memory(
        model_spec: &str,
        instruction: Option<&str>,
        inference: &entheai_config::InferenceConfig,
        providers: &HashMap<String, ProviderConfig>,
        registry: &entheai_tools::ToolRegistry,
        policy: Arc<Policy>,
        prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
        max_iterations: u32,
        memory: Arc<entheai_memory::MemoryRuntime>,
        pp: Option<Arc<entheai_memory_pp::PromptProcessor>>,
        scope: entheai_memory::MemoryScope,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::AgentEvent>>,
    ) -> anyhow::Result<Self> {
        Self::build(
            model_spec,
            instruction,
            inference,
            providers,
            registry,
            policy,
            prompter,
            max_iterations,
            Some((memory, pp, scope, event_tx)),
        )
    }

    /// Picks [`Self::new_with_memory`] or [`Self::new_with_instruction`]
    /// based on whether `memory` is present — the "build whichever
    /// `EntheaiAgent` variant this turn needs" branch that both the TUI's
    /// per-turn interactive spawn and bin/entheai's one-shot path used to
    /// duplicate independently. `pp`/`scope`/`event_tx` are only consulted
    /// when `memory` is `Some` (matching `new_with_memory`'s own contract).
    #[allow(clippy::too_many_arguments)]
    pub fn build_auto(
        model_spec: &str,
        instruction: Option<&str>,
        inference: &entheai_config::InferenceConfig,
        providers: &HashMap<String, ProviderConfig>,
        registry: &entheai_tools::ToolRegistry,
        policy: Arc<Policy>,
        prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
        max_iterations: u32,
        memory: Option<Arc<entheai_memory::MemoryRuntime>>,
        pp: Option<Arc<entheai_memory_pp::PromptProcessor>>,
        scope: entheai_memory::MemoryScope,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::AgentEvent>>,
    ) -> anyhow::Result<Self> {
        match memory {
            Some(memory) => Self::new_with_memory(
                model_spec,
                instruction,
                inference,
                providers,
                registry,
                policy,
                prompter,
                max_iterations,
                memory,
                pp,
                scope,
                event_tx,
            ),
            None => Self::new_with_instruction(
                model_spec,
                instruction,
                inference,
                providers,
                registry,
                policy,
                prompter,
                max_iterations,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        model_spec: &str,
        instruction: Option<&str>,
        inference: &entheai_config::InferenceConfig,
        providers: &HashMap<String, ProviderConfig>,
        registry: &entheai_tools::ToolRegistry,
        policy: Arc<Policy>,
        prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
        max_iterations: u32,
        memory_ctx: Option<MemoryCtx>,
    ) -> anyhow::Result<Self> {
        let model = crate::model_resolve::resolve_model(model_spec, providers)?;

        let mut builder = LlmAgentBuilder::new("entheai")
            .model(model)
            .max_iterations(max_iterations);
        if let Some(instruction) = instruction {
            builder = builder.instruction(instruction);
        }
        if let Some(temperature) = inference.temperature {
            builder = builder.temperature(temperature);
        }
        if let Some(max_tokens) = inference.max_tokens {
            builder = builder.max_output_tokens(clamp_max_tokens(max_tokens));
        }
        for tool in registry.to_tools() {
            let adapter = crate::adk_tool_adapter::AdkToolAdapter::new(
                tool,
                Arc::clone(&policy),
                Arc::clone(&prompter),
            );
            builder = builder.tool(Arc::new(adapter));
        }
        let mut injected_sessions: Option<Arc<tokio::sync::Mutex<HashSet<String>>>> = None;
        if let Some((memory, pp, scope, event_tx)) = memory_ctx {
            let injected = Arc::new(tokio::sync::Mutex::new(HashSet::new()));
            builder = builder
                .before_model_callback(crate::memory_callbacks::before_model_retrieval_callback(
                    Arc::clone(&memory),
                    pp.clone(),
                    Arc::clone(&injected),
                    event_tx,
                ))
                .after_tool_callback_full(crate::memory_callbacks::after_tool_evidence_callback(
                    scope, memory, pp,
                ));
            // Kept on the agent so `cleanup_run` can drop the per-session
            // marker once a run's stream has been consumed (see
            // `run_to_text` and `event_bridge::run_with_events`).
            injected_sessions = Some(injected);
        }
        let agent: Arc<dyn adk_rust::Agent> = Arc::new(builder.build()?);

        let app_name = "entheai".to_string();
        let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let runner = Runner::builder()
            .app_name(app_name.clone())
            .agent(agent)
            .session_service(Arc::clone(&sessions))
            .build()?;

        Ok(Self {
            runner,
            sessions,
            app_name,
            injected_sessions,
            // `0` is a common "disable the timeout" config convention, but
            // `Duration::ZERO` makes every stream idle-timeout fire
            // immediately (see `request_timeout`'s doc comment) — clamp to a
            // minimum of 1s instead of letting 0 mean "never wait at all".
            request_timeout: std::time::Duration::from_secs(inference.request_timeout_secs.max(1)),
        })
    }

    /// Streaming entry point that seeds a fresh session with prior
    /// `(role, text)` turns (`role` is `"user"` or `"assistant"`) before
    /// running `user_message`, so the model sees the full conversation.
    ///
    /// Seeds via `SessionService::append_event` — confirmed (empirically,
    /// against a mocked endpoint, since `Session::conversation_history`'s own
    /// implementation wasn't traceable in the vendored source) that appended
    /// events are read back into `LlmRequest.contents` on the next
    /// `run_str` call for the same session, exactly like real prior turns.
    ///
    /// Returns the fresh per-run `session_id` alongside the stream so the
    /// caller can release the session (via [`Self::cleanup_run`]) once the
    /// stream has been fully consumed.
    pub async fn run_with_history(
        &self,
        prior_turns: &[(Arc<str>, Arc<str>)],
        user_message: &str,
    ) -> anyhow::Result<(String, adk_rust::EventStream)> {
        let session_id = uuid::Uuid::new_v4().to_string();
        self.sessions
            .create(CreateRequest {
                app_name: self.app_name.clone(),
                user_id: "entheai".to_string(),
                session_id: Some(session_id.clone()),
                state: HashMap::new(),
            })
            .await?;

        for (role, text) in prior_turns {
            let adk_role = if role.as_ref() == "assistant" {
                "model"
            } else {
                role.as_ref()
            };
            let mut ev = adk_rust::Event::new(&session_id);
            ev.author = adk_role.to_string();
            ev.llm_response.content = Some(Content::new(adk_role).with_text(text.to_string()));
            self.sessions.append_event(&session_id, ev).await?;
        }

        let stream = match self
            .runner
            .run_str(
                "entheai",
                &session_id,
                Content::new("user").with_text(user_message),
            )
            .await
        {
            Ok(stream) => stream,
            Err(e) => {
                // The session was created above; drop it so a failed start
                // doesn't leave it behind in the session service.
                self.cleanup_run(&session_id).await;
                return Err(e.into());
            }
        };
        Ok((session_id, stream))
    }

    /// Test/CLI convenience: collect the stream into the final assistant text.
    /// Uses the last non-partial, pure-text event (no tool calls/results
    /// alongside it — mirrors `event_bridge::run_with_events`'s stricter
    /// candidate-final-answer filter, so a model that emits preamble text
    /// alongside a tool call isn't mistaken for having finished) as the answer.
    pub async fn run_to_text(&self, user_message: &str) -> anyhow::Result<String> {
        use adk_rust::Part;
        use futures::StreamExt;

        let (session_id, mut stream) = self.run_with_history(&[], user_message).await?;
        let mut text = String::new();
        // Partial (streamed-delta) accumulation for providers that stream only
        // partials and never emit a final non-partial text event (Ollama-backed
        // free tiers). Mirrors `event_bridge::run_with_events`; reset at every
        // turn boundary so it only holds the LAST turn's stream.
        let mut streamed = String::new();
        let mut stream_err: Option<anyhow::Error> = None;
        // Per-event-gap idle timeout on the provider: 2× the configured
        // value. Suspended while a tool call is in flight (FunctionCall seen,
        // FunctionResponse not yet) — that window is tool/permission time, not
        // provider idleness; mirrors `event_bridge::run_with_events`.
        let idle = self.request_timeout.saturating_mul(2);
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
            if ev.llm_response.partial {
                if let Some(content) = ev.content() {
                    for part in &content.parts {
                        if let Some(t) = part.text() {
                            streamed.push_str(t);
                        }
                    }
                }
                continue;
            }
            if let Some(content) = ev.content() {
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
                let joined: String = content.parts.iter().filter_map(|p| p.text()).collect();
                if !joined.is_empty() && !has_calls && !has_results {
                    text = joined;
                    streamed.clear(); // a real final answer arrived — drop partials
                } else if has_calls || has_results {
                    // Turn boundary: partials before this were thinking-text
                    // ahead of a tool call/round, not the final answer.
                    streamed.clear();
                }
            }
        }
        self.cleanup_run(&session_id).await;
        if let Some(e) = stream_err {
            return Err(e);
        }
        // Fallback: providers that stream only partial deltas and never emit a
        // final non-partial text event (see `streamed` above).
        if text.is_empty() && !streamed.is_empty() {
            text = streamed;
        }
        Ok(text)
    }

    /// Releases a run's per-run session state once its stream has been fully
    /// consumed (or failed): drops the `injected_sessions` before-model marker
    /// (memory path) and deletes the session from the session service. Every
    /// run creates a fresh uuid session, so without this the session service
    /// and the marker set grow without bound on long-lived agent reuse.
    /// Called from [`Self::run_to_text`] and `crate::event_bridge`'s
    /// `run_with_events`.
    pub(crate) async fn cleanup_run(&self, session_id: &str) {
        if let Some(injected) = &self.injected_sessions {
            injected.lock().await.remove(session_id);
        }
        let _ = self
            .sessions
            .delete(DeleteRequest {
                app_name: self.app_name.clone(),
                user_id: "entheai".to_string(),
                session_id: session_id.to_string(),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_final_answer_server(answer: &str) -> MockServer {
        let server = MockServer::start().await;
        let body = format!(
            "data: {{\"id\":\"t\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"\"}},\"finish_reason\":null}}]}}\n\n\
             data: {{\"id\":\"t\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{answer}\"}},\"finish_reason\":\"stop\"}}]}}\n\n\
             data: [DONE]\n\n"
        );
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .insert_header("Content-Type", "text/event-stream"),
            )
            .mount(&server)
            .await;
        server
    }

    struct AllowAll;
    #[async_trait::async_trait]
    impl Prompter for AllowAll {
        async fn confirm(&mut self, _tool: &str, _args: &str) -> entheai_permission::Grant {
            entheai_permission::Grant::Allow
        }
    }

    // Regression: `max_tokens` is a `u32` in config but `max_output_tokens`
    // takes an `i32` — an oversized value used to wrap negative on the cast.
    #[test]
    fn max_tokens_is_clamped_to_i32_max_before_cast() {
        assert_eq!(clamp_max_tokens(25), 25);
        assert_eq!(clamp_max_tokens(i32::MAX as u32), i32::MAX);
        assert_eq!(clamp_max_tokens(i32::MAX as u32 + 1), i32::MAX);
        assert_eq!(clamp_max_tokens(u32::MAX), i32::MAX);
    }

    // Regression: `request_timeout_secs = 0` (a common "disable" convention)
    // used to become `Duration::ZERO`, making every stream idle-timeout fire
    // instantly instead of never/rarely.
    #[test]
    fn zero_request_timeout_secs_is_clamped_to_a_minimum() {
        let inference = entheai_config::InferenceConfig {
            request_timeout_secs: 0,
            ..Default::default()
        };
        let mut providers = HashMap::new();
        providers.insert(
            "test".to_string(),
            ProviderConfig {
                base_url: "http://localhost:8000/v1".to_string(),
                api_key_env: None,
                ..Default::default()
            },
        );
        let agent = EntheaiAgent::new_with_instruction(
            "test/model",
            None,
            &inference,
            &providers,
            &entheai_tools::ToolRegistry::new(),
            Arc::new(Policy::new(true, vec![])),
            Arc::new(tokio::sync::Mutex::new(AllowAll)),
            25,
        )
        .expect("agent builds");
        assert!(
            agent.request_timeout() >= std::time::Duration::from_secs(1),
            "expected the zero timeout to be clamped, got {:?}",
            agent.request_timeout()
        );
    }

    #[tokio::test]
    async fn run_to_text_returns_final_answer_with_no_tools() {
        let server = mock_final_answer_server("final answer").await;
        let mut providers = HashMap::new();
        providers.insert(
            "test".to_string(),
            ProviderConfig {
                base_url: server.uri(),
                api_key_env: None,
                ..Default::default()
            },
        );

        let agent = EntheaiAgent::new(
            "test/model",
            &providers,
            &entheai_tools::ToolRegistry::new(),
            Arc::new(Policy::new(true, vec![])),
            Arc::new(tokio::sync::Mutex::new(AllowAll)),
            25,
        )
        .expect("agent builds");

        let text = agent.run_to_text("hello").await.expect("run succeeds");
        assert_eq!(text, "final answer");
    }

    #[tokio::test]
    async fn before_model_callback_injects_retrieval_brief_into_request() {
        use entheai_memory::{
            Memory, MemoryRuntime, MemoryRuntimeConfig, MemoryScope, Namespace, SqliteStore,
        };
        use wiremock::matchers::body_string_contains;

        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(
                Namespace::Codebase,
                "k1",
                "the auth module lives in crates/permission",
                None,
            )
            .await
            .unwrap();
        let memory = Arc::new(MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig {
                enabled: true,
                ..Default::default()
            },
        ));

        let server = MockServer::start().await;
        let body = "data: {\"id\":\"t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ack\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_string_contains("auth module"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .insert_header("Content-Type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let mut providers = HashMap::new();
        providers.insert(
            "test".to_string(),
            ProviderConfig {
                base_url: server.uri(),
                api_key_env: None,
                ..Default::default()
            },
        );

        let agent = EntheaiAgent::new_with_memory(
            "test/model",
            None,
            &entheai_config::InferenceConfig::default(),
            &providers,
            &entheai_tools::ToolRegistry::new(),
            Arc::new(Policy::new(true, vec![])),
            Arc::new(tokio::sync::Mutex::new(AllowAll)),
            25,
            memory,
            None,
            MemoryScope {
                session_id: "s1".into(),
                task_id: "t1".into(),
                cwd: std::env::temp_dir(),
                role: None,
            },
            None,
        )
        .expect("agent builds");

        // Fails with a 404-from-wiremock-style error if the injected brief
        // never reached the outbound request body — the mock only matches
        // requests whose body contains "auth module".
        let text = agent
            .run_to_text("where does the auth module live?")
            .await
            .expect("run succeeds, proving the mock matched the injected request body");
        assert_eq!(text, "ack");
    }
}
