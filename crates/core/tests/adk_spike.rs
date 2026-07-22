use std::collections::HashMap;
use std::sync::Arc;

use adk_rust::agent::LlmAgentBuilder;
use adk_rust::model::openai::{OpenAIClient, OpenAIConfig};
use adk_rust::runner::Runner;
use adk_rust::serde_json::{json, Value};
use adk_rust::session::{CreateRequest, InMemorySessionService, SessionService};
use adk_rust::tool::FunctionTool;
use adk_rust::{AdkError, Content, Result as AdkResult, Tool, ToolContext};
use futures::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Deserialize, Serialize, JsonSchema)]
struct EchoArgs {
    text: String,
}

async fn echo(_ctx: Arc<dyn ToolContext>, args: Value) -> AdkResult<Value> {
    let echo_args: EchoArgs = serde_json::from_value(args)
        .map_err(|e| AdkError::tool(format!("failed to deserialize EchoArgs: {e}")))?;
    Ok(json!({ "echoed": echo_args.text }))
}

#[tokio::test]
async fn spike_llm_agent_runner_session_roundtrip() {
    let server = MockServer::start().await;
    // Non-tool-calling chat-completions response: the model just answers directly.
    // Exact response body shape confirmed against OpenAI-compatible chat/completions
    // format already used by crates/providers' own tests (see
    // crates/providers/src/lib.rs's `complete_handles_plain_text_answer` test for
    // the reference JSON shape) — reuse that same fixture shape here.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_string("data: {\"id\":\"spike-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"spike-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"spike ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
            .insert_header("Content-Type", "text/event-stream")
        )
        .mount(&server)
        .await;

    let config = OpenAIConfig::compatible("no-key", server.uri(), "spike-model");
    let model = Arc::new(OpenAIClient::new(config).expect("client builds"));

    let tool: Arc<dyn Tool> = Arc::new(
        FunctionTool::new("echo", "Echo text back.", echo).with_parameters_schema::<EchoArgs>(),
    );

    let agent = Arc::new(
        LlmAgentBuilder::new("spike-agent")
            .instruction("You are a test agent.")
            .model(model)
            .tool(tool)
            .max_iterations(5)
            .build()
            .expect("agent builds"),
    );

    let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    sessions
        .create(CreateRequest {
            app_name: "entheai-spike".into(),
            user_id: "spike-user".into(),
            session_id: Some("spike-session".into()),
            state: HashMap::new(),
        })
        .await
        .expect("session creates");

    let runner = Runner::builder()
        .app_name("entheai-spike")
        .agent(agent)
        .session_service(sessions)
        .build()
        .expect("runner builds");

    let mut stream = runner
        .run_str(
            "spike-user",
            "spike-session",
            Content::new("user").with_text("say hi"),
        )
        .await
        .expect("run starts");

    let mut final_text = String::new();
    while let Some(ev) = stream.next().await {
        let ev = ev.expect("no stream error");
        if let Some(content) = ev.content() {
            for part in &content.parts {
                if let Some(t) = part.text() {
                    final_text.push_str(t);
                }
            }
        }
    }
    assert!(
        final_text.contains("spike ok"),
        "expected mocked reply, got {final_text:?}"
    );
}
