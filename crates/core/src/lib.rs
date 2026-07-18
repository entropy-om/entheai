use entheai_providers::{ChatMessage, Provider, StreamEvent};
use futures::StreamExt;

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
    ) -> anyhow::Result<String> {
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
    ) -> anyhow::Result<String> {
        // Hard cap on tool-dispatch rounds, so a looping model can't burn unbounded
        // paid API calls (critical under --yolo, where no human approves each call).
        const MAX_TURNS: usize = 25;
        let schemas = registry.schemas();
        for _turn in 0..MAX_TURNS {
            let resp = self
                .provider
                .complete(&self.model, messages.clone(), schemas.clone())
                .await?;
            if resp.tool_calls.is_empty() {
                return Ok(resp.content);
            }
            // Record the assistant's tool-call message in history.
            messages.push(ChatMessage::assistant_tool_calls(resp.tool_calls.clone()));
            for call in resp.tool_calls {
                let result = self.dispatch_call(&call, registry, policy, prompter).await;
                messages.push(ChatMessage::tool_result(call.id, result));
            }
        }
        anyhow::bail!("run_task exceeded {MAX_TURNS} tool-dispatch turns without a final answer")
    }

    async fn dispatch_call(
        &self,
        call: &entheai_providers::ToolCall,
        registry: &entheai_tools::ToolRegistry,
        policy: &entheai_permission::Policy,
        prompter: &mut impl entheai_permission::Prompter,
    ) -> String {
        use entheai_permission::Decision;
        let name = &call.function.name;
        let allowed = match policy.decide(name) {
            Decision::Allow => true,
            Decision::Deny => false,
            Decision::Ask => prompter.confirm(name, &call.function.arguments),
        };
        if !allowed {
            return format!("error: permission denied for tool '{name}'");
        }
        let Some(tool) = registry.get(name) else {
            return format!("error: unknown tool '{name}'");
        };
        let args: serde_json::Value = match serde_json::from_str(&call.function.arguments) {
            Ok(v) => v,
            Err(e) => return format!("error: could not parse tool arguments as JSON: {e}"),
        };
        match tool.call(args).await {
            Ok(out) => out,
            Err(e) => format!("error: {e}"),
        }
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
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
            let mut evs: Vec<anyhow::Result<StreamEvent>> = self
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
        ) -> anyhow::Result<entheai_providers::AssistantResponse> {
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
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            _msgs: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> anyhow::Result<AssistantResponse> {
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
        async fn call(&self, args: serde_json::Value) -> anyhow::Result<String> {
            Ok(format!("echoed: {}", args["text"].as_str().unwrap_or("")))
        }
    }

    struct AllowAll;
    impl Prompter for AllowAll {
        fn confirm(&mut self, _t: &str, _a: &str) -> bool {
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
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            _msgs: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> anyhow::Result<AssistantResponse> {
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
            )
            .await;
        assert!(result.is_err());
        assert!(format!("{}", result.err().unwrap()).contains("exceeded"));
    }
}
