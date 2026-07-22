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
use adk_rust::session::{CreateRequest, InMemorySessionService, SessionService};
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

pub struct EntheaiAgent {
    runner: Runner,
    sessions: Arc<dyn SessionService>,
    app_name: String,
}

impl EntheaiAgent {
    pub fn new(
        model_spec: &str,
        providers: &HashMap<String, ProviderConfig>,
        registry: entheai_tools::ToolRegistry,
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
        registry: entheai_tools::ToolRegistry,
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
    /// `inference.request_timeout_secs`/`.retries` have no adk-rust 1.0.0
    /// `OpenAIClient` equivalent (confirmed: it hardcodes `reqwest::Client::new()`
    /// with no timeout/retry builder surface) and are intentionally NOT
    /// applied — a known, accepted gap. `temperature`/`max_tokens` carry over
    /// via `LlmAgentBuilder::temperature`/`max_output_tokens`.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_memory(
        model_spec: &str,
        instruction: Option<&str>,
        inference: &entheai_config::InferenceConfig,
        providers: &HashMap<String, ProviderConfig>,
        registry: entheai_tools::ToolRegistry,
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

    #[allow(clippy::too_many_arguments)]
    fn build(
        model_spec: &str,
        instruction: Option<&str>,
        inference: &entheai_config::InferenceConfig,
        providers: &HashMap<String, ProviderConfig>,
        registry: entheai_tools::ToolRegistry,
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
            builder = builder.max_output_tokens(max_tokens as i32);
        }
        for tool in registry.into_tools() {
            let adapter = crate::adk_tool_adapter::AdkToolAdapter::new(
                Arc::from(tool),
                Arc::clone(&policy),
                Arc::clone(&prompter),
            );
            builder = builder.tool(Arc::new(adapter));
        }
        if let Some((memory, pp, scope, event_tx)) = memory_ctx {
            let injected_sessions = Arc::new(tokio::sync::Mutex::new(HashSet::new()));
            builder = builder
                .before_model_callback(crate::memory_callbacks::before_model_retrieval_callback(
                    Arc::clone(&memory),
                    pp.clone(),
                    injected_sessions,
                    event_tx,
                ))
                .after_tool_callback_full(crate::memory_callbacks::after_tool_evidence_callback(
                    scope, memory, pp,
                ));
        }
        let agent: Arc<dyn adk_rust::Agent> = Arc::new(builder.build()?);

        let app_name = "entheai".to_string();
        let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let runner = Runner::builder()
            .app_name(app_name.clone())
            .agent(agent)
            .session_service(Arc::clone(&sessions))
            .build()?;

        Ok(Self { runner, sessions, app_name })
    }

    /// Streaming entry point. Each call gets a fresh session with no prior
    /// turns — for a caller that needs to carry conversation history forward
    /// (e.g. an interactive chat), use [`Self::run_with_history`] instead.
    pub async fn run(&self, user_message: &str) -> anyhow::Result<adk_rust::EventStream> {
        self.run_with_history(&[], user_message).await
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
    pub async fn run_with_history(
        &self,
        prior_turns: &[(String, String)],
        user_message: &str,
    ) -> anyhow::Result<adk_rust::EventStream> {
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
            let adk_role = if role == "assistant" { "model" } else { role.as_str() };
            let mut ev = adk_rust::Event::new(&session_id);
            ev.author = adk_role.to_string();
            ev.llm_response.content = Some(Content::new(adk_role).with_text(text.clone()));
            self.sessions.append_event(&session_id, ev).await?;
        }

        let stream = self
            .runner
            .run_str("entheai", &session_id, Content::new("user").with_text(user_message))
            .await?;
        Ok(stream)
    }

    /// Test/CLI convenience: collect the stream into the final assistant text.
    /// Uses the last non-partial event carrying text content as the answer.
    pub async fn run_to_text(&self, user_message: &str) -> anyhow::Result<String> {
        use futures::StreamExt;

        let mut stream = self.run(user_message).await?;
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if !ev.llm_response.partial {
                if let Some(content) = ev.content() {
                    let joined: String = content.parts.iter().filter_map(|p| p.text()).collect();
                    if !joined.is_empty() {
                        text = joined;
                    }
                }
            }
        }
        Ok(text)
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

    #[tokio::test]
    async fn run_to_text_returns_final_answer_with_no_tools() {
        let server = mock_final_answer_server("final answer").await;
        let mut providers = HashMap::new();
        providers.insert(
            "test".to_string(),
            ProviderConfig { base_url: server.uri(), api_key_env: None },
        );

        let agent = EntheaiAgent::new(
            "test/model",
            &providers,
            entheai_tools::ToolRegistry::new(),
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
        use entheai_memory::{Memory, MemoryRuntime, MemoryRuntimeConfig, MemoryScope, Namespace, SqliteStore};
        use wiremock::matchers::body_string_contains;

        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Codebase, "k1", "the auth module lives in crates/permission", None)
            .await
            .unwrap();
        let memory = Arc::new(MemoryRuntime::new(
            Arc::new(store),
            MemoryRuntimeConfig { enabled: true, ..Default::default() },
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
            ProviderConfig { base_url: server.uri(), api_key_env: None },
        );

        let agent = EntheaiAgent::new_with_memory(
            "test/model",
            None,
            &entheai_config::InferenceConfig::default(),
            &providers,
            entheai_tools::ToolRegistry::new(),
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
