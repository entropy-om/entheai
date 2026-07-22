//! `EntheaiAgent` — the adk-rust-backed replacement for `Agent<P>::run_task`.
//! See docs/superpowers/plans/2026-07-22-adk-rust-core-migration.md, Task 4.
//!
//! `adk_runner::Runner::session_service` is a private field with no public
//! accessor (confirmed against the vendored adk-runner 1.0.0 source), so this
//! wrapper holds its own `Arc<dyn SessionService>` alongside the `Runner`
//! rather than trying to recover it from the runner after construction.

use std::collections::HashMap;
use std::sync::Arc;

use adk_rust::agent::LlmAgentBuilder;
use adk_rust::runner::Runner;
use adk_rust::session::{CreateRequest, InMemorySessionService, SessionService};
use adk_rust::Content;
use entheai_config::ProviderConfig;
use entheai_permission::{Policy, Prompter};

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
        let model = crate::model_resolve::resolve_model(model_spec, providers)?;

        let mut builder = LlmAgentBuilder::new("entheai")
            .model(model)
            .max_iterations(max_iterations);
        for tool in registry.into_tools() {
            let adapter = crate::adk_tool_adapter::AdkToolAdapter::new(
                Arc::from(tool),
                Arc::clone(&policy),
                Arc::clone(&prompter),
            );
            builder = builder.tool(Arc::new(adapter));
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

    /// Streaming entry point. Each call gets a fresh session — entheai's own
    /// callers hold the full conversation history upstream of this call, the
    /// same way `Agent<P>::run_task` took a fresh `Vec<ChatMessage>` each time.
    pub async fn run(&self, user_message: &str) -> anyhow::Result<adk_rust::EventStream> {
        let session_id = uuid::Uuid::new_v4().to_string();
        self.sessions
            .create(CreateRequest {
                app_name: self.app_name.clone(),
                user_id: "entheai".to_string(),
                session_id: Some(session_id.clone()),
                state: HashMap::new(),
            })
            .await?;
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
}
