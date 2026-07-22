pub mod adk_tool_adapter;
pub mod model_resolve;

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
    /// A frozen node was woken and activated.
    FrozenWoke { name: String },
}

/// Deadline for activating a frozen node (500 ms).
const FROZEN_ACTIVATE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

/// Where streamed tokens go (stdout in the CLI, the TUI later).
pub trait TokenSink {
    fn emit(&mut self, token: &str);
}

pub struct Agent<P: Provider> {
    provider: P,
    model: String,
    max_turns: usize,
}

impl<P: Provider> Agent<P> {
    pub fn new(provider: P, model: String) -> Self {
        Self {
            provider,
            model,
            max_turns: 25,
        }
    }

    /// Override the per-task tool-dispatch round cap (default 25).
    pub fn with_max_turns(mut self, n: usize) -> Self {
        self.max_turns = n.max(1);
        self
    }

    /// Run one turn: stream the model's reply to `sink`, return the full text.
    pub async fn run_turn(
        &self,
        messages: Vec<ChatMessage>,
        sink: &mut impl TokenSink,
    ) -> Result<String, CoreError> {
        let mut stream = self.provider.stream_chat(&self.model, &messages).await?;
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

    /// Emit `Thinking`, stream one model completion (forwarding text deltas as
    /// `AgentEvent::Token` as they arrive), and return the assembled response.
    /// Shared by `run_task` and `run_task_with_memory` to keep the streaming
    /// select-loop in exactly one place.
    async fn stream_turn(
        &self,
        messages: &[ChatMessage],
        schemas: &[serde_json::Value],
        events: &Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<entheai_providers::AssistantResponse, CoreError> {
        if let Some(tx) = events {
            let _ = tx.send(AgentEvent::Thinking);
        }
        let (ttx, mut trx) = futures::channel::mpsc::unbounded::<String>();
        let completion = self
            .provider
            .stream_complete(&self.model, messages, schemas, Some(ttx));
        tokio::pin!(completion);
        loop {
            tokio::select! {
                biased;
                Some(tok) = trx.next() => {
                    if let Some(tx) = events { let _ = tx.send(AgentEvent::Token(tok)); }
                }
                r = &mut completion => {
                    // drain tokens buffered right before the future resolved
                    while let Ok(tok) = trx.try_recv() {
                        if let Some(tx) = events { let _ = tx.send(AgentEvent::Token(tok)); }
                    }
                    break Ok(r?);
                }
            }
        }
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
        let schemas = registry.schemas();
        for _turn in 0..self.max_turns {
            let resp = self.stream_turn(&messages, &schemas, &events).await?;
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
                let dr = self
                    .dispatch_call(&call, registry, policy, prompter, &events)
                    .await;
                messages.push(ChatMessage::tool_result(call.id, dr.result));
            }
        }
        Err(CoreError::MaxTurnsExceeded(self.max_turns))
    }

    /// Agentic loop with memory awareness. Injects pre-task retrieval context,
    /// spills large tool outputs, and records post-task trajectory + learnings.
    ///
    /// When `memory` is `None`, behaves identically to [`run_task`].
    #[allow(clippy::too_many_arguments)]
    pub async fn run_task_with_memory(
        &self,
        mut messages: Vec<ChatMessage>,
        registry: &entheai_tools::ToolRegistry,
        policy: &entheai_permission::Policy,
        prompter: &mut impl entheai_permission::Prompter,
        events: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
        memory: Option<&entheai_memory::MemoryRuntime>,
        pp: Option<&entheai_memory_pp::PromptProcessor>,
        scope: entheai_memory::MemoryScope,
    ) -> Result<String, CoreError> {
        // Pre-task: inject retrieval context if enabled. `injected_ctx` is declared
        // outside the guard so the transcript-ingest hook can exclude the brief we
        // injected (avoiding a self-reinforcing recall loop across sessions).
        let mut injected_ctx: Option<String> = None;
        if let Some(mem) = memory {
            // Insert the retrieved context immediately BEFORE the last user
            // message (the turn it was retrieved for). Using the user message's
            // actual index — not `len - 1` — keeps it correct when the list ends
            // with a non-user message (e.g. a resumed conversation ending in a
            // tool/assistant turn).
            if let Some(user_idx) = messages.iter().rposition(|m| m.role == "user") {
                let user_msg = messages[user_idx].content.clone();
                // Dispatch: prompt-processing when configured + present; else
                // today's top-K. The fallback arm calls the UNCHANGED
                // retrieve_before with the SAME query, so a fallback is
                // byte-identical to today's behaviour.
                //
                // Correction #5 / Slice-1 note: PP's stubs always fall back here,
                // and the processor logs the failing stage (→ stderr in oneshot,
                // the only Slice-1 mode). A user-facing fallback notice via the
                // `events` channel is deferred to Slice 2, where fallback is the
                // exception rather than the rule — emitting one per prompt while
                // the mesh is stubbed would be pure noise.
                let retrieved: Result<Option<String>, entheai_memory::MemoryError> = match pp {
                    Some(p) => match p.retrieve(&user_msg).await {
                        Ok(Some(brief)) => Ok(Some(brief)),
                        Ok(None) | Err(_) => mem.retrieve_before(&user_msg).await,
                    },
                    None => mem.retrieve_before(&user_msg).await,
                };
                match retrieved {
                    Ok(Some(ctx)) => {
                        injected_ctx = Some(ctx.clone());
                        messages.insert(user_idx, ChatMessage::system(ctx));
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

        // PP frozen-node wake/activate: runs unconditionally on `pp` regardless
        // of whether `memory` is `Some`.
        if let Some(p) = pp {
            if let Some(user_idx) = messages.iter().rposition(|m| m.role == "user") {
                let user_msg = messages[user_idx].content.clone();
                for node in p.wake_frozen(&user_msg, 1) {
                    let brief = p.activate_frozen(&node, FROZEN_ACTIVATE_DEADLINE).await;
                    messages.insert(user_idx, ChatMessage::system(brief));
                    if let Some(tx) = &events {
                        let _ = tx.send(AgentEvent::FrozenWoke {
                            name: node.name.clone(),
                        });
                    }
                }
            }
        }

        let schemas = registry.schemas();
        let mut tool_evidence: Vec<entheai_memory::ToolEvidence> = Vec::new();

        for _turn in 0..self.max_turns {
            let resp = self.stream_turn(&messages, &schemas, &events).await?;
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
                // Phase-1 transcript ingest (best-effort), cleaned of the brief we
                // injected so memory's own context is never recalled into future briefs.
                if let Some(p) = pp {
                    let clean = transcript_for_ingest(&messages, injected_ctx.as_deref());
                    p.ingest_transcript(&scope, &clean, &resp.content).await;
                }
                return Ok(resp.content);
            }
            messages.push(ChatMessage::assistant_tool_calls(
                resp.content.clone(),
                resp.tool_calls.clone(),
            ));
            for call in resp.tool_calls {
                let dr = self
                    .dispatch_call(&call, registry, policy, prompter, &events)
                    .await;

                // Tool spillover.
                if let Some(mem) = memory {
                    let ev = entheai_memory::ToolEvidence {
                        call_id: call.id.clone(),
                        name: call.function.name.clone(),
                        args: call.function.arguments.clone(),
                        result: dr.result.clone(),
                        allowed: dr.allowed,
                    };
                    // Phase-1 raw ingest: unconditional, ahead of memory's spill gate.
                    if let Some(p) = pp {
                        p.ingest_tool(&scope, &ev).await;
                    }
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

                messages.push(ChatMessage::tool_result(call.id, dr.result));
            }
        }
        Err(CoreError::MaxTurnsExceeded(self.max_turns))
    }

    async fn dispatch_call(
        &self,
        call: &entheai_providers::ToolCall,
        registry: &entheai_tools::ToolRegistry,
        policy: &entheai_permission::Policy,
        prompter: &mut impl entheai_permission::Prompter,
        events: &Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    ) -> DispatchResult {
        use entheai_permission::{Decision, Grant};
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

        let tier = registry
            .get(name)
            .map(|t| t.tier())
            .unwrap_or(entheai_permission::Tier::Exec);
        let allowed = match policy.decide_tiered(name, tier) {
            Decision::Allow => true,
            Decision::Deny => false,
            Decision::Ask => match prompter.confirm(name, &call.function.arguments).await {
                Grant::Deny => false,
                Grant::Allow => true,
                Grant::AllowSession => {
                    policy.grant_session(name);
                    true
                }
            },
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
        DispatchResult { result, allowed }
    }
}

/// Outcome of a single tool dispatch, used by both `run_task` and
/// `run_task_with_memory`.
struct DispatchResult {
    result: String,
    allowed: bool,
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

/// Build the transcript to raw-ingest, excluding the memory-context system
/// message we injected before the user turn (identified by exact content match).
/// Without this, `ingest_transcript` would re-ingest memory's own injected brief,
/// which would then be recalled and compressed into future briefs — a
/// self-reinforcing loop that degrades retrieval quality across sessions.
fn transcript_for_ingest(
    messages: &[entheai_providers::ChatMessage],
    injected_ctx: Option<&str>,
) -> Vec<entheai_providers::ChatMessage> {
    messages
        .iter()
        .filter(|m| !(m.role == "system" && injected_ctx == Some(m.content.as_str())))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use entheai_providers::{ChatMessage, Provider, StreamEvent};
    use futures::stream::{self, BoxStream};

    #[test]
    fn transcript_for_ingest_drops_only_injected_ctx() {
        let injected = "Memory context:\n\n[codebase score=0.90 key=k]\nbody\n";
        let messages = vec![
            ChatMessage::system("you are helpful"), // real system prompt — kept
            ChatMessage::system(injected.to_string()), // memory's injected brief — dropped
            ChatMessage::user("do the thing"),
        ];
        let clean = transcript_for_ingest(&messages, Some(injected));
        assert_eq!(clean.len(), 2);
        assert!(
            clean.iter().all(|m| m.content != injected),
            "injected ctx filtered out"
        );
        assert!(
            clean.iter().any(|m| m.content == "you are helpful"),
            "real system prompt kept"
        );

        // No injection this turn → nothing dropped.
        let clean2 = transcript_for_ingest(&messages, None);
        assert_eq!(clean2.len(), 3);
    }

    struct MockProvider {
        tokens: Vec<&'static str>,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn stream_chat(
            &self,
            _model: &str,
            _messages: &[ChatMessage],
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
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
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

    use entheai_permission::{Decision, Grant, Policy, Prompter};
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
            _msgs: &[ChatMessage],
        ) -> Result<
            BoxStream<'static, Result<StreamEvent, entheai_providers::ProviderError>>,
            entheai_providers::ProviderError,
        > {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            _msgs: &[ChatMessage],
            _tools: &[serde_json::Value],
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
        async fn confirm(&mut self, _t: &str, _a: &str) -> Grant {
            Grant::Allow
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
        let policy = Policy::new(true, vec![]);
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

    struct FinalAnswerProvider;
    #[async_trait]
    impl Provider for FinalAnswerProvider {
        async fn stream_chat(
            &self,
            _m: &str,
            _msgs: &[ChatMessage],
        ) -> Result<
            BoxStream<'static, Result<StreamEvent, entheai_providers::ProviderError>>,
            entheai_providers::ProviderError,
        > {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            _msgs: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> Result<AssistantResponse, entheai_providers::ProviderError> {
            Ok(AssistantResponse {
                content: "final answer".into(),
                tool_calls: vec![],
            })
        }
    }

    fn test_scope() -> entheai_memory::MemoryScope {
        entheai_memory::MemoryScope {
            session_id: "sess".into(),
            task_id: "task".into(),
            cwd: std::path::PathBuf::from("/tmp"),
            role: None,
        }
    }

    #[tokio::test]
    async fn run_task_with_memory_wakes_frozen_node_and_emits_event() {
        use entheai_memory_pp::frozen::{FrozenNode, FrozenStore};
        use entheai_memory_pp::{PromptProcessor, RawStore, StubMarqant, StubMesh};

        let node = FrozenNode {
            name: "nixos".into(),
            domain: "cloud".into(),
            triggers: vec!["hetzner".into()],
            mcp: None,
            rank: 1.0,
            knowledge: "use nix flakes".into(),
        };
        let raw = RawStore::open_memory().unwrap();
        let pp = PromptProcessor::new(
            raw,
            Box::new(StubMesh),
            Box::new(StubMarqant),
            std::time::Duration::from_millis(50),
            16,
            1 << 20,
            FrozenStore::from_nodes(vec![node]),
        );

        let agent = Agent::new(FinalAnswerProvider, "m".into());
        let registry = ToolRegistry::new();
        let policy = Policy::new(true, vec![]);
        let mut prompter = AllowAll;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

        let answer = agent
            .run_task_with_memory(
                vec![ChatMessage::user("please deploy to hetzner")],
                &registry,
                &policy,
                &mut prompter,
                Some(tx),
                None,
                Some(&pp),
                test_scope(),
            )
            .await
            .unwrap();
        assert_eq!(answer, "final answer");

        let mut saw_wake = false;
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::FrozenWoke { name } = ev {
                assert_eq!(name, "nixos");
                saw_wake = true;
            }
        }
        assert!(saw_wake, "expected a FrozenWoke event for the matched node");
    }

    #[tokio::test]
    async fn run_task_with_memory_no_frozen_match_emits_nothing() {
        use entheai_memory_pp::frozen::FrozenStore;
        use entheai_memory_pp::{PromptProcessor, RawStore, StubMarqant, StubMesh};

        let raw = RawStore::open_memory().unwrap();
        let pp = PromptProcessor::new(
            raw,
            Box::new(StubMesh),
            Box::new(StubMarqant),
            std::time::Duration::from_millis(50),
            16,
            1 << 20,
            FrozenStore::from_nodes(vec![]),
        );

        let agent = Agent::new(FinalAnswerProvider, "m".into());
        let registry = ToolRegistry::new();
        let policy = Policy::new(true, vec![]);
        let mut prompter = AllowAll;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

        let _ = agent
            .run_task_with_memory(
                vec![ChatMessage::user("unrelated question")],
                &registry,
                &policy,
                &mut prompter,
                Some(tx),
                None,
                Some(&pp),
                test_scope(),
            )
            .await
            .unwrap();

        let mut saw_wake = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, AgentEvent::FrozenWoke { .. }) {
                saw_wake = true;
            }
        }
        assert!(!saw_wake, "no trigger matched, no FrozenWoke event expected");
    }

    struct AlwaysToolProvider;
    #[async_trait]
    impl Provider for AlwaysToolProvider {
        async fn stream_chat(
            &self,
            _m: &str,
            _msgs: &[ChatMessage],
        ) -> Result<
            BoxStream<'static, Result<StreamEvent, entheai_providers::ProviderError>>,
            entheai_providers::ProviderError,
        > {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            _msgs: &[ChatMessage],
            _tools: &[serde_json::Value],
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
        let policy = Policy::new(true, vec![]);
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
    async fn max_turns_caps_the_loop() {
        // AlwaysToolProvider ALWAYS returns a tool call (never a final text
        // answer), so it would loop forever; with_max_turns(1) must stop after
        // ONE dispatch round with MaxTurnsExceeded(1).
        let provider = AlwaysToolProvider;
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let policy = Policy::new(true, vec![]);
        let mut prompter = AllowAll;
        let agent = Agent::new(provider, "m".to_string()).with_max_turns(1);
        let err = agent
            .run_task(
                vec![ChatMessage::user("go")],
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::MaxTurnsExceeded(1)));
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
        let policy = Policy::new(true, vec![]);
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
            _msgs: &[ChatMessage],
        ) -> Result<
            BoxStream<'static, Result<StreamEvent, entheai_providers::ProviderError>>,
            entheai_providers::ProviderError,
        > {
            Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::Done)])))
        }
        async fn complete(
            &self,
            _m: &str,
            msgs: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> Result<AssistantResponse, entheai_providers::ProviderError> {
            self.seen.lock().unwrap().push(msgs.to_vec());
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
        async fn confirm(&mut self, _t: &str, _a: &str) -> Grant {
            Grant::Deny
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
        let policy = Policy::new(false, vec![]);
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
        let policy = Policy::new(true, vec![]);
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
        let policy = Policy::new(true, vec![]);
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

    fn tool_call(tool_name: &str, args: &str) -> Vec<AssistantResponse> {
        vec![AssistantResponse {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: tool_name.into(),
                    arguments: args.into(),
                },
            }],
        }]
    }

    fn final_answer(s: &str) -> Vec<AssistantResponse> {
        vec![AssistantResponse {
            content: s.into(),
            tool_calls: vec![],
        }]
    }

    struct CountingSessionPrompter {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait]
    impl Prompter for CountingSessionPrompter {
        async fn confirm(&mut self, _t: &str, _a: &str) -> entheai_permission::Grant {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            entheai_permission::Grant::AllowSession
        }
    }

    #[tokio::test]
    async fn allow_for_session_stops_reprompting() {
        // provider: echo tool call, echo tool call, final answer.
        let provider = RecordingProvider {
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(
                vec![
                    tool_call("echo", "{}"),
                    tool_call("echo", "{}"),
                    final_answer("done"),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ),
        };
        let agent = Agent::new(provider, "m".into());
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let policy = Policy::new(false, vec![]); // non-yolo -> Ask
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut prompter = CountingSessionPrompter {
            calls: calls.clone(),
        };
        let ans = agent
            .run_task(
                vec![ChatMessage::user("go")],
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await
            .unwrap();
        assert_eq!(ans, "done");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "second echo must not re-prompt"
        );
    }

    struct ReadTool;
    #[async_trait]
    impl Tool for ReadTool {
        fn name(&self) -> &str {
            "read_tool"
        }
        fn tier(&self) -> entheai_permission::Tier {
            entheai_permission::Tier::Read
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"function","function":{"name":"read_tool","parameters":{"type":"object","properties":{}}}})
        }
        async fn call(&self, _args: serde_json::Value) -> Result<String, entheai_tools::ToolError> {
            Ok("read_data".into())
        }
    }

    struct WriteTool;
    #[async_trait]
    impl Tool for WriteTool {
        fn name(&self) -> &str {
            "write_tool"
        }
        fn tier(&self) -> entheai_permission::Tier {
            entheai_permission::Tier::Write
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"function","function":{"name":"write_tool","parameters":{"type":"object","properties":{}}}})
        }
        async fn call(&self, _args: serde_json::Value) -> Result<String, entheai_tools::ToolError> {
            Ok("wrote".into())
        }
    }

    #[tokio::test]
    async fn plan_mode_denies_writes_but_allows_reads() {
        let provider = RecordingProvider {
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(vec![
                AssistantResponse {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "read_tool".into(),
                            arguments: "{}".into(),
                        },
                    }],
                },
                AssistantResponse {
                    content: "final answer".into(),
                    tool_calls: vec![],
                },
            ]),
        };
        let agent = Agent::new(provider, "m".into());
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool));
        registry.register(Box::new(WriteTool));
        let policy = Policy::with_mode(entheai_permission::Mode::Plan);
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
            tool_msg.content.contains("read_data"),
            "plan mode must allow read_tool, got: {}",
            tool_msg.content
        );
    }
}
