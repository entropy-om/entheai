use entheai_providers::{ChatMessage, Provider, StreamEvent};
use futures::StreamExt;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Provider(#[from] entheai_providers::ProviderError),
    #[error("run_task exceeded {0} tool-dispatch turns without a final answer")]
    MaxTurnsExceeded(usize),
    #[error("memory error: {0}")]
    Memory(String),
}

/// Progress notifications emitted by `run_task` while it works, so a UI (e.g.
/// the TUI) can render a live "what's happening" indicator without polling.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// About to call the model.
    Thinking,
    /// About to execute a tool.
    ToolStarted { name: String, args: String },
    /// A tool call returned (or was denied/failed — `result` carries the
    /// "error: …" text in that case, same string fed back to the model).
    ToolFinished { name: String, result: String },
    /// A text delta streamed live from the model.
    Token(String),
}

/// Where streamed tokens go (stdout in the CLI, the TUI later).
pub trait TokenSink {
    fn emit(&mut self, token: &str);
}

pub struct Agent<P: Provider> {
    provider: P,
    model: String,
}

impl<P: Provider> Agent<P> {
    pub fn new(provider: P, model: String) -> Self {
        Self { provider, model }
    }

    /// Run one turn: stream the model's reply to `sink`, return the full text.
    pub async fn run_turn(
        &self,
        messages: Vec<ChatMessage>,
        sink: &mut impl TokenSink,
    ) -> Result<String, CoreError> {
        let mut stream = self.provider.stream_chat(&self.model, messages).await?;
        let mut full = String::new();
        while let Some(ev) = stream.next().await {
            match ev? {
                StreamEvent::Token(t) => {
                    full.push_str(&t);
                    sink.emit(&t);
                }
                StreamEvent::Done => break,
            }
        }
        Ok(full)
    }

    /// Agentic loop: repeatedly `complete()` with the tool schemas; execute any
    /// tool calls (gated by `policy`/`prompter`) and feed results back until the
    /// model answers with no tool calls. Returns the final text answer.
    pub async fn run_task(
        &self,
        mut messages: Vec<ChatMessage>,
        registry: &entheai_tools::ToolRegistry,
        policy: &entheai_permission::Policy,
        prompter: &mut impl entheai_permission::Prompter,
        events: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<String, CoreError> {
        // Hard cap on tool-dispatch rounds, so a looping model can't burn unbounded
        // paid API calls (critical under --yolo, where no human approves each call).
        const MAX_TURNS: usize = 25;
        let schemas = registry.schemas();
        for _turn in 0..MAX_TURNS {
            if let Some(tx) = &events {
                let _ = tx.send(AgentEvent::Thinking);
            }
            let (ttx, mut trx) = futures::channel::mpsc::unbounded::<String>();
            let completion = self.provider.stream_complete(
                &self.model,
                messages.clone(),
                schemas.clone(),
                Some(ttx),
            );
            tokio::pin!(completion);
            let resp = loop {
                tokio::select! {
                    biased;
                    Some(tok) = trx.next() => {
                        if let Some(tx) = &events { let _ = tx.send(AgentEvent::Token(tok)); }
                    }
                    r = &mut completion => {
                        // drain any tokens buffered right before the future resolved
                        while let Ok(tok) = trx.try_recv() {
                            if let Some(tx) = &events { let _ = tx.send(AgentEvent::Token(tok)); }
                        }
                        break r?;
                    }
                }
            };
            if resp.tool_calls.is_empty() {
                return Ok(resp.content);
            }
            // Record the assistant's tool-call message in history, preserving any
            // reasoning text the model emitted alongside the tool calls.
            messages.push(ChatMessage::assistant_tool_calls(
                resp.content.clone(),
                resp.tool_calls.clone(),
            ));
            for call in resp.tool_calls {
                let result = self
                    .dispatch_call(&call, registry, policy, prompter, &events)
                    .await;
                messages.push(ChatMessage::tool_result(call.id, result));
            }
        }
        Err(CoreError::MaxTurnsExceeded(MAX_TURNS))
    }

    /// Agentic loop with memory awareness. Injects pre-task retrieval context,
    /// spills large tool outputs, and records post-task trajectory + learnings.
    ///
    /// When `memory` is `None`, behaves identically to [`run_task`].
    pub async fn run_task_with_memory(
        &self,
        mut messages: Vec<ChatMessage>,
        registry: &entheai_tools::ToolRegistry,
        policy: &entheai_permission::Policy,
        prompter: &mut impl entheai_permission::Prompter,
        events: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
        memory: Option<&entheai_memory::MemoryRuntime>,
        scope: entheai_memory::MemoryScope,
    ) -> Result<String, CoreError> {
        // Pre-task: inject memory context if enabled.
        if let Some(mem) = memory {
            if let Some(user_msg) = messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| m.content.clone())
            {
                match mem.retrieve_before(&user_msg).await {
                    Ok(Some(ctx)) => {
                        messages.insert(
                            messages.len().saturating_sub(1), // before last user msg
                            ChatMessage::system(ctx),
                        );
                    }
                    Ok(None) => {}
                    Err(e) => {
                        if mem.config().strict {
                            return Err(CoreError::Memory(e.to_string()));
                        }
                    }
                }
            }
        }

        const MAX_TURNS: usize = 25;
        let schemas = registry.schemas();
        let mut tool_evidence: Vec<entheai_memory::ToolEvidence> = Vec::new();

        for _turn in 0..MAX_TURNS {
            if let Some(tx) = &events {
                let _ = tx.send(AgentEvent::Thinking);
            }
            let (ttx, mut trx) = futures::channel::mpsc::unbounded::<String>();
            let completion = self.provider.stream_complete(
                &self.model,
                messages.clone(),
                schemas.clone(),
                Some(ttx),
            );
            tokio::pin!(completion);
            let resp = loop {
                tokio::select! {
                    biased;
                    Some(tok) = trx.next() => {
                        if let Some(tx) = &events { let _ = tx.send(AgentEvent::Token(tok)); }
                    }
                    r = &mut completion => {
                        while let Ok(tok) = trx.try_recv() {
                            if let Some(tx) = &events { let _ = tx.send(AgentEvent::Token(tok)); }
                        }
                        break r?;
                    }
                }
            };
            if resp.tool_calls.is_empty() {
                // Post-task: record final answer.
                if let Some(mem) = memory {
                    let preview = truncate_preview(&resp.content, 500);
                    if let Err(e) = mem
                        .record_final_answer(&scope, &self.model, &preview, &tool_evidence)
                        .await
                    {
                        if mem.config().strict {
                            return Err(CoreError::Memory(e.to_string()));
                        }
                    }
                }
                return Ok(resp.content);
            }
            messages.push(ChatMessage::assistant_tool_calls(
                resp.content.clone(),
                resp.tool_calls.clone(),
            ));
            for call in resp.tool_calls {
                let result = self
                    .dispatch_call(&call, registry, policy, prompter, &events)
                    .await;

                // Tool spillover.
                if let Some(mem) = memory {
                    let ev = entheai_memory::ToolEvidence {
                        call_id: call.id.clone(),
                        name: call.function.name.clone(),
                        args: call.function.arguments.clone(),
                        result: result.clone(),
                        allowed: true,
                    };
                    match mem.record_tool_result(&scope, &ev).await {
                        Ok(Some(pointer)) => {
                            messages.push(ChatMessage::tool_result(call.id, pointer));
                            tool_evidence.push(ev);
                            continue; // skip pushing the full result
                        }
                        Ok(None) => {}
                        Err(e) => {
                            if mem.config().strict {
                                return Err(CoreError::Memory(e.to_string()));
                            }
                        }
                    }
                    tool_evidence.push(ev);
                }

                messages.push(ChatMessage::tool_result(call.id, result));
            }
        }
        Err(CoreError::MaxTurnsExceeded(MAX_TURNS))
    }

    async fn dispatch_call(
        &self,
        call: &entheai_providers::ToolCall,
        registry: &entheai_tools::ToolRegistry,
        policy: &entheai_permission::Policy,
        prompter: &mut impl entheai_permission::Prompter,
        events: &Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    ) -> String {
        use entheai_permission::Decision;
        let name = &call.function.name;

        // Emit ToolStarted up front so the UI can show "about to run X" even if
        // the call is ultimately denied/unknown/malformed — ToolFinished below
        // always follows with whatever result string (including "error: …")
        // ends up fed back to the model.
        if let Some(tx) = events {
            let _ = tx.send(AgentEvent::ToolStarted {
                name: name.clone(),
                args: call.function.arguments.clone(),
            });
        }

        let allowed = match policy.decide(name) {
            Decision::Allow => true,
            Decision::Deny => false,
            Decision::Ask => prompter.confirm(name, &call.function.arguments).await,
        };
        let result = if !allowed {
            format!("error: permission denied for tool '{name}'")
        } else if let Some(tool) = registry.get(name) {
            match serde_json::from_str(&call.function.arguments) {
                Ok(args) => match tool.call(args).await {
                    Ok(out) => out,
                    Err(e) => format!("error: {e}"),
                },
                Err(e) => format!("error: could not parse tool arguments as JSON: {e}"),
            }
        } else {
            format!("error: unknown tool '{name}'")
        };

        if let Some(tx) = events {
            let _ = tx.send(AgentEvent::ToolFinished {
                name: name.clone(),
                result: result.clone(),
            });
        }
        result
    }
}

/// Truncate a string to `max` chars, appending `…` if cut.
fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use entheai_providers::{ChatMessage, Provider, StreamEvent};
    use futures::stream::{self, BoxStream};

    struct MockProvider {
        tokens: Vec<&'static str>,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn stream_chat(
            &self,
            _model: &str,
            _messages: Vec<ChatMessage>,
        ) -> Result<
            BoxStream<'static, Result<StreamEvent, entheai_providers::ProviderError>>,
            entheai_providers::ProviderError,
        > {
            let mut evs: Vec<Result<StreamEvent, entheai_providers::ProviderError>> = self
                .tokens
                .iter()
                .map(|t| Ok(StreamEvent::Token((*t).to_string())))
                .collect();
            evs.push(Ok(StreamEvent::Done));
            Ok(Box::pin(stream::iter(evs)))
        }

        async fn complete(
            &self,
            _model: &str,
            _messages: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<entheai_providers::AssistantResponse, entheai_providers::ProviderError>
        {
            Ok(entheai_providers::AssistantResponse::default())
        }
    }

    struct CollectSink(String);
    impl TokenSink for CollectSink {
        fn emit(&mut self, token: &str) {
            self.0.push_str(token);
        }
    }

    #[tokio::test]
    async fn run_turn_streams_and_returns_full_text() {
        let agent = Agent::new(
            MockProvider {
                tokens: vec!["Hel", "lo"],
            },
            "m".into(),
        );
        let mut sink = CollectSink(String::new());
        let msgs = vec![ChatMessage::user("hi")];
        let full = agent.run_turn(msgs, &mut sink).await.unwrap();
        assert_eq!(full, "Hello");
        assert_eq!(sink.0, "Hello");
    }

    use entheai_permission::{Decision, Policy, Prompter};
    use entheai_providers::{AssistantResponse, FunctionCall, ToolCall};
    use entheai_tools::{Tool, ToolRegistry};
    use std::sync::Mutex;

    // Provider that returns a tool call on the first `complete`, then a final answer.
    struct ScriptedProvider {
        calls: Mutex<usize>,
    }
    #[async_trait]
    impl Provider for ScriptedProvider {
        async fn stream_chat(
            &self,
            _m: &str,
            _msgs: Vec<ChatMessage>,
        ) -> Result<
            BoxStream<'static, Result<StreamEvent, entheai_providers::ProviderError>>,
            entheai_providers::ProviderError,
        > {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            _msgs: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<AssistantResponse, entheai_providers::ProviderError> {
            let mut n = self.calls.lock().unwrap();
            *n += 1;
            if *n == 1 {
                Ok(AssistantResponse {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "echo".into(),
                            arguments: "{\"text\":\"hi\"}".into(),
                        },
                    }],
                })
            } else {
                Ok(AssistantResponse {
                    content: "final answer".into(),
                    tool_calls: vec![],
                })
            }
        }
    }

    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"function","function":{"name":"echo","parameters":{"type":"object","properties":{}}}})
        }
        async fn call(&self, args: serde_json::Value) -> Result<String, entheai_tools::ToolError> {
            Ok(format!("echoed: {}", args["text"].as_str().unwrap_or("")))
        }
    }

    struct AllowAll;
    #[async_trait]
    impl Prompter for AllowAll {
        async fn confirm(&mut self, _t: &str, _a: &str) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn run_task_dispatches_tool_then_returns_final_answer() {
        let agent = Agent::new(
            ScriptedProvider {
                calls: Mutex::new(0),
            },
            "m".into(),
        );
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let policy = Policy {
            yolo: true,
            allowlist: vec![],
        };
        let mut prompter = AllowAll;

        let answer = agent
            .run_task(
                vec![ChatMessage::user("do it")],
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await
            .unwrap();
        assert_eq!(answer, "final answer");
        assert_eq!(policy.decide("echo"), Decision::Allow); // sanity
    }

    struct AlwaysToolProvider;
    #[async_trait]
    impl Provider for AlwaysToolProvider {
        async fn stream_chat(
            &self,
            _m: &str,
            _msgs: Vec<ChatMessage>,
        ) -> Result<
            BoxStream<'static, Result<StreamEvent, entheai_providers::ProviderError>>,
            entheai_providers::ProviderError,
        > {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            _msgs: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<AssistantResponse, entheai_providers::ProviderError> {
            Ok(AssistantResponse {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "c".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "echo".into(),
                        arguments: "{}".into(),
                    },
                }],
            })
        }
    }

    #[tokio::test]
    async fn run_task_caps_runaway_tool_loops() {
        let agent = Agent::new(AlwaysToolProvider, "m".into());
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let policy = Policy {
            yolo: true,
            allowlist: vec![],
        };
        let mut prompter = AllowAll;
        let result = agent
            .run_task(
                vec![ChatMessage::user("loop")],
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await;
        assert!(result.is_err());
        assert!(format!("{}", result.err().unwrap()).contains("exceeded"));
    }

    #[tokio::test]
    async fn run_task_emits_thinking_and_tool_events() {
        let agent = Agent::new(
            ScriptedProvider {
                calls: Mutex::new(0),
            },
            "m".into(),
        );
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let policy = Policy {
            yolo: true,
            allowlist: vec![],
        };
        let mut prompter = AllowAll;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let answer = agent
            .run_task(
                vec![ChatMessage::user("do it")],
                &registry,
                &policy,
                &mut prompter,
                Some(tx),
            )
            .await
            .unwrap();
        assert_eq!(answer, "final answer");

        let mut received = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            received.push(ev);
        }

        assert!(matches!(received[0], AgentEvent::Thinking));
        assert!(matches!(
            &received[1],
            AgentEvent::ToolStarted { name, .. } if name == "echo"
        ));
        assert!(matches!(
            &received[2],
            AgentEvent::ToolFinished { name, .. } if name == "echo"
        ));
        assert!(matches!(received[3], AgentEvent::Thinking));
    }

    /// A provider that records every `messages` vec it's called with (so tests
    /// can inspect exactly what gets fed back after a tool dispatch), and walks
    /// through a scripted sequence of responses: a single tool call, then a
    /// final answer.
    struct RecordingProvider {
        seen: Mutex<Vec<Vec<ChatMessage>>>,
        responses: Mutex<Vec<AssistantResponse>>,
    }
    #[async_trait]
    impl Provider for RecordingProvider {
        async fn stream_chat(
            &self,
            _m: &str,
            _msgs: Vec<ChatMessage>,
        ) -> Result<
            BoxStream<'static, Result<StreamEvent, entheai_providers::ProviderError>>,
            entheai_providers::ProviderError,
        > {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            msgs: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<AssistantResponse, entheai_providers::ProviderError> {
            self.seen.lock().unwrap().push(msgs);
            let mut responses = self.responses.lock().unwrap();
            Ok(responses.remove(0))
        }
    }

    fn tool_call_then_final(tool_name: &str, args: &str) -> Vec<AssistantResponse> {
        vec![
            AssistantResponse {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: tool_name.into(),
                        arguments: args.into(),
                    },
                }],
            },
            AssistantResponse {
                content: "final answer".into(),
                tool_calls: vec![],
            },
        ]
    }

    struct DenyAll;
    #[async_trait]
    impl Prompter for DenyAll {
        async fn confirm(&mut self, _t: &str, _a: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn run_task_feeds_back_permission_denied_tool_result() {
        let provider = RecordingProvider {
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(tool_call_then_final("echo", "{\"text\":\"hi\"}")),
        };
        let agent = Agent::new(provider, "m".into());
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        // Non-yolo policy with no allowlist -> `decide` asks the prompter, which
        // denies everything.
        let policy = Policy {
            yolo: false,
            allowlist: vec![],
        };
        let mut prompter = DenyAll;

        let answer = agent
            .run_task(
                vec![ChatMessage::user("do it")],
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await
            .unwrap();
        assert_eq!(answer, "final answer");

        // The second `complete()` call sees the tool-result message fed back
        // after dispatch; assert it carries the permission-denied error.
        let seen = agent.provider.seen.lock().unwrap();
        let second_call_messages = &seen[1];
        let tool_msg = second_call_messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("expected a tool-result message");
        assert!(
            tool_msg.content.contains("permission denied"),
            "expected permission denied, got: {}",
            tool_msg.content
        );
    }

    #[tokio::test]
    async fn run_task_feeds_back_unknown_tool_error() {
        let provider = RecordingProvider {
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(tool_call_then_final("does_not_exist", "{}")),
        };
        let agent = Agent::new(provider, "m".into());
        // Registry has no tools registered at all.
        let registry = ToolRegistry::new();
        let policy = Policy {
            yolo: true,
            allowlist: vec![],
        };
        let mut prompter = AllowAll;

        let answer = agent
            .run_task(
                vec![ChatMessage::user("do it")],
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await
            .unwrap();
        assert_eq!(answer, "final answer");

        let seen = agent.provider.seen.lock().unwrap();
        let second_call_messages = &seen[1];
        let tool_msg = second_call_messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("expected a tool-result message");
        assert!(
            tool_msg.content.contains("unknown tool"),
            "expected unknown tool error, got: {}",
            tool_msg.content
        );
    }

    #[tokio::test]
    async fn run_task_feeds_back_bad_json_args_error() {
        let provider = RecordingProvider {
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(tool_call_then_final("echo", "{not json")),
        };
        let agent = Agent::new(provider, "m".into());
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let policy = Policy {
            yolo: true,
            allowlist: vec![],
        };
        let mut prompter = AllowAll;

        let answer = agent
            .run_task(
                vec![ChatMessage::user("do it")],
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await
            .unwrap();
        assert_eq!(answer, "final answer");

        let seen = agent.provider.seen.lock().unwrap();
        let second_call_messages = &seen[1];
        let tool_msg = second_call_messages
            .iter()
            .find(|m| m.role == "tool")
            .expect("expected a tool-result message");
        assert!(
            tool_msg.content.contains("could not parse tool arguments"),
            "expected bad JSON args error, got: {}",
            tool_msg.content
        );
    }
}
