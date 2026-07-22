//! Parity tests: the six `Agent<P>::run_task` behaviors from the pre-migration
//! test suite, ported against `EntheaiAgent`/`event_bridge::run_with_events`.
//! See docs/superpowers/plans/2026-07-22-adk-rust-core-migration.md, Task 8.
//!
//! Two of the six don't port as literal scenario+assertion pairs — the
//! underlying library behavior changed, not just the API surface:
//!
//! - `run_task_feeds_back_unknown_tool_error`: adk-agent's own tool-dispatch
//!   loop (not `AdkToolAdapter`) produces the "not found" error for an
//!   unregistered tool name — different wording than the old
//!   `"error: unknown tool '{name}'"`, but the same graceful-feedback shape.
//! - `run_task_feeds_back_bad_json_args_error`: adk-model's OpenAI SSE parser
//!   (`openai_compatible.rs`) does `serde_json::from_str(&args_str).unwrap_or(json!({}))`
//!   on the accumulated tool-call arguments — malformed JSON silently becomes
//!   `{}`, not an error, and this happens upstream of both `AdkToolAdapter`
//!   and adk-agent's own loop. There is no error to feed back; the test below
//!   asserts the actual (confirmed, accepted) new behavior instead.

use std::collections::HashMap;
use std::sync::Arc;

use entheai_config::ProviderConfig;
use entheai_core::entheai_agent::EntheaiAgent;
use entheai_core::event_bridge::run_with_events;
use entheai_core::AgentEvent;
use entheai_permission::{Grant, Policy, Prompter};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct AllowAll;
#[async_trait::async_trait]
impl Prompter for AllowAll {
    async fn confirm(&mut self, _tool: &str, _args: &str) -> Grant {
        Grant::Allow
    }
}

struct DenyAll;
#[async_trait::async_trait]
impl Prompter for DenyAll {
    async fn confirm(&mut self, _tool: &str, _args: &str) -> Grant {
        Grant::Deny
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
                    "properties": { "text": { "type": "string" } }
                }
            }
        })
    }
    async fn call(&self, args: serde_json::Value) -> Result<String, entheai_tools::ToolError> {
        Ok(format!("echoed: {}", args["text"].as_str().unwrap_or("")))
    }
}

fn providers(base_url: String) -> HashMap<String, ProviderConfig> {
    let mut m = HashMap::new();
    m.insert("test".to_string(), ProviderConfig { base_url, api_key_env: None });
    m
}

fn build_agent(
    server: &MockServer,
    registry: &entheai_tools::ToolRegistry,
    policy: Arc<Policy>,
    prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
    max_iterations: u32,
) -> EntheaiAgent {
    EntheaiAgent::new_with_instruction(
        "test/model",
        None,
        &entheai_config::InferenceConfig::default(),
        &providers(server.uri()),
        registry,
        policy,
        prompter,
        max_iterations,
    )
    .expect("agent builds")
}

fn tool_call_sse(id: &str, name: &str, args_json: &str) -> String {
    format!(
        "data: {{\"id\":\"t\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"tool_calls\":[{{\"index\":0,\"id\":\"{id}\",\"type\":\"function\",\"function\":{{\"name\":\"{name}\",\"arguments\":\"\"}}}}]}},\"finish_reason\":null}}]}}\n\n\
         data: {{\"id\":\"t\",\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"arguments\":{args_json}}}}}]}},\"finish_reason\":null}}]}}\n\n\
         data: {{\"id\":\"t\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n\
         data: [DONE]\n\n"
    )
}

fn final_answer_sse(answer: &str) -> String {
    format!(
        "data: {{\"id\":\"t\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{answer}\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
    )
}

fn sse_template(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(body).insert_header("Content-Type", "text/event-stream")
}

#[tokio::test]
async fn run_task_dispatches_tool_then_returns_final_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(tool_call_sse("call_1", "echo", "\"{\\\"text\\\":\\\"hi\\\"}\"")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(final_answer_sse("final answer")))
        .mount(&server)
        .await;

    let mut registry = entheai_tools::ToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let agent = build_agent(
        &server,
        &registry,
        Arc::new(Policy::new(true, vec![])),
        Arc::new(tokio::sync::Mutex::new(AllowAll)),
        25,
    );

    let answer = agent.run_to_text("do it").await.unwrap();
    assert_eq!(answer, "final answer");
}

#[tokio::test]
async fn run_task_caps_runaway_tool_loops() {
    let server = MockServer::start().await;
    // Always returns a tool call, never a final answer — a model stuck in a loop.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(tool_call_sse("call_1", "echo", "\"{}\"")))
        .mount(&server)
        .await;

    let mut registry = entheai_tools::ToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let agent = build_agent(
        &server,
        &registry,
        Arc::new(Policy::new(true, vec![])),
        Arc::new(tokio::sync::Mutex::new(AllowAll)),
        2, // low cap so the test runs fast
    );

    let err = agent.run_to_text("loop").await.unwrap_err();
    assert!(
        format!("{err}").contains("exceeded"),
        "expected a max-iterations-exceeded error, got: {err}"
    );
}

#[tokio::test]
async fn run_task_emits_thinking_and_tool_events() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(tool_call_sse("call_1", "echo", "\"{\\\"text\\\":\\\"hi\\\"}\"")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(final_answer_sse("final answer")))
        .mount(&server)
        .await;

    let mut registry = entheai_tools::ToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let agent = build_agent(
        &server,
        &registry,
        Arc::new(Policy::new(true, vec![])),
        Arc::new(tokio::sync::Mutex::new(AllowAll)),
        25,
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let answer = run_with_events(&agent, &[], "do it", "test/model", tx, None, None, test_scope())
        .await
        .unwrap();
    assert_eq!(answer, "final answer");

    let mut received = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        received.push(ev);
    }
    assert!(matches!(received[0], AgentEvent::Thinking));
    assert!(matches!(&received[1], AgentEvent::ToolStarted { name, .. } if name == "echo"));
    assert!(matches!(&received[2], AgentEvent::ToolFinished { name, .. } if name == "echo"));
    assert!(matches!(received[3], AgentEvent::Thinking));
}

#[tokio::test]
async fn run_task_feeds_back_permission_denied_tool_result() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(tool_call_sse("call_1", "echo", "\"{\\\"text\\\":\\\"hi\\\"}\"")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // The second call carries the tool result back; it must contain the
    // permission-denied error for this mock (matched on body) to fire.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("permission denied"))
        .respond_with(sse_template(final_answer_sse("final answer")))
        .mount(&server)
        .await;

    let mut registry = entheai_tools::ToolRegistry::new();
    registry.register(Box::new(EchoTool));
    // Non-yolo policy with no allowlist -> `decide` asks the prompter, which denies everything.
    let agent = build_agent(
        &server,
        &registry,
        Arc::new(Policy::new(false, vec![])),
        Arc::new(tokio::sync::Mutex::new(DenyAll)),
        25,
    );

    let answer = agent
        .run_to_text("do it")
        .await
        .expect("run succeeds, proving the mock matched the permission-denied tool result");
    assert_eq!(answer, "final answer");
}

#[tokio::test]
async fn run_task_feeds_back_unknown_tool_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(tool_call_sse("call_1", "does_not_exist", "\"{}\"")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // adk-agent's own dispatch loop (not AdkToolAdapter) produces "not found"
    // for a tool name it never registered — different wording than the old
    // "unknown tool", same graceful-feedback behavior.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("not found"))
        .respond_with(sse_template(final_answer_sse("final answer")))
        .mount(&server)
        .await;

    // Registry has no tools registered at all.
    let registry = entheai_tools::ToolRegistry::new();
    let agent = build_agent(
        &server,
        &registry,
        Arc::new(Policy::new(true, vec![])),
        Arc::new(tokio::sync::Mutex::new(AllowAll)),
        25,
    );

    let answer = agent
        .run_to_text("do it")
        .await
        .expect("run succeeds, proving the mock matched the 'not found' tool result");
    assert_eq!(answer, "final answer");
}

#[tokio::test]
async fn run_task_feeds_back_bad_json_args_error() {
    let server = MockServer::start().await;
    // Malformed JSON in the tool-call arguments.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(tool_call_sse("call_1", "echo", "\"{not json\"")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_template(final_answer_sse("final answer")))
        .mount(&server)
        .await;

    let mut registry = entheai_tools::ToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let agent = build_agent(
        &server,
        &registry,
        Arc::new(Policy::new(true, vec![])),
        Arc::new(tokio::sync::Mutex::new(AllowAll)),
        25,
    );

    // Confirmed new behavior (adk-model's openai_compatible.rs): malformed
    // JSON args silently fall back to `{}` rather than surfacing a parse
    // error — the tool still runs, just with no arguments. There is no
    // "could not parse tool arguments" feedback to assert on anymore.
    let answer = agent
        .run_to_text("do it")
        .await
        .expect("run succeeds even with malformed tool-call args (silently -> {})");
    assert_eq!(answer, "final answer");
}

fn test_scope() -> entheai_memory::MemoryScope {
    entheai_memory::MemoryScope {
        session_id: "sess".into(),
        task_id: "task".into(),
        cwd: std::path::PathBuf::from("/tmp"),
        role: None,
    }
}
