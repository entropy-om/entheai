use std::sync::Arc;

use adk_rust::serde_json::{json, Value};
use adk_rust::{
    async_trait, CallbackContext, Content, Result as AdkResult, Tool as AdkTool, ToolContext,
};
use entheai_permission::{Decision, Policy, Prompter};
use entheai_tools::Tool;
use tokio::sync::Mutex;

/// Wraps an `entheai_tools::Tool` (and its `entheai_permission` policy +
/// prompter) behind the `adk_rust::Tool` trait so it can be passed to the ADK
/// agent runner.
pub struct AdkToolAdapter {
    inner: Arc<dyn Tool>,
    policy: Arc<Policy>,
    prompter: Arc<Mutex<dyn Prompter>>,
}

impl AdkToolAdapter {
    pub fn new(
        inner: Arc<dyn Tool>,
        policy: Arc<Policy>,
        prompter: Arc<Mutex<dyn Prompter>>,
    ) -> Self {
        Self {
            inner,
            policy,
            prompter,
        }
    }
}

#[async_trait]
impl AdkTool for AdkToolAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        ""
    }

    fn declaration(&self) -> Value {
        self.inner.schema()
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, args: Value) -> AdkResult<Value> {
        let tool_name = self.inner.name().to_string();

        // Follow the plan's permission logic (same flow as Agent::dispatch_call).
        let tier = self.inner.tier();
        let allowed = match self.policy.decide_tiered(&tool_name, tier) {
            Decision::Allow => true,
            Decision::Deny => false,
            Decision::Ask => {
                let summary = args.to_string();
                let grant = self
                    .prompter
                    .lock()
                    .await
                    .confirm(&tool_name, &summary)
                    .await;
                match grant {
                    entheai_permission::Grant::Deny => false,
                    entheai_permission::Grant::Allow => true,
                    entheai_permission::Grant::AllowSession => {
                        self.policy.grant_session(&tool_name);
                        true
                    }
                }
            }
        };

        if !allowed {
            return Ok(json!({ "error": "permission denied" }));
        }

        match self.inner.call(args).await {
            Ok(text) => Ok(json!({ "result": text })),
            Err(e) => {
                // Return a JSON error value (not an Err) so the LLM can see it.
                Ok(json!({ "error": e.to_string() }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_rust::{
        async_trait, EventActions, MemoryEntry, ReadonlyContext, Result as AdkResult,
    };
    use entheai_permission::{Grant, Pin, Policy, Prompter};
    use entheai_tools::Tool;
    use std::sync::Arc;

    // ------------------------------------------------------------------
    // NoopToolContext — a test-only context with trivial return values.
    // ------------------------------------------------------------------
    struct NoopToolContext;

    #[async_trait]
    impl ReadonlyContext for NoopToolContext {
        fn invocation_id(&self) -> &str {
            "test"
        }
        fn agent_name(&self) -> &str {
            "test"
        }
        fn user_id(&self) -> &str {
            "test"
        }
        fn app_name(&self) -> &str {
            "test"
        }
        fn session_id(&self) -> &str {
            "test"
        }
        fn branch(&self) -> &str {
            "main"
        }
        fn user_content(&self) -> &Content {
            static CONTENT: std::sync::LazyLock<Content> =
                std::sync::LazyLock::new(|| Content::new("user").with_text("test"));
            &CONTENT
        }
    }

    #[async_trait]
    impl CallbackContext for NoopToolContext {
        fn artifacts(&self) -> Option<Arc<dyn adk_rust::Artifacts>> {
            None
        }
    }

    #[async_trait]
    impl ToolContext for NoopToolContext {
        fn function_call_id(&self) -> &str {
            "fc_1"
        }
        fn actions(&self) -> EventActions {
            EventActions::default()
        }
        fn set_actions(&self, _actions: EventActions) {
            // no-op
        }
        async fn search_memory(&self, _query: &str) -> AdkResult<Vec<MemoryEntry>> {
            Ok(vec![])
        }
    }

    /// Constructor for tests.
    pub fn noop_tool_context() -> Arc<dyn ToolContext> {
        Arc::new(NoopToolContext)
    }

    // ------------------------------------------------------------------
    // EchoTool — minimal tool implementation for testing.
    // ------------------------------------------------------------------
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"function","function":{"name":"echo","parameters":{"type":"object","properties":{}}}})
        }
        async fn call(
            &self,
            args: serde_json::Value,
        ) -> Result<String, entheai_tools::ToolError> {
            Ok(format!("echoed: {}", args["text"].as_str().unwrap_or("")))
        }
    }

    // ------------------------------------------------------------------
    // AlwaysDeny — a prompter that always denies.
    // ------------------------------------------------------------------
    struct AlwaysDeny;

    #[async_trait]
    impl Prompter for AlwaysDeny {
        async fn confirm(&mut self, _tool: &str, _args: &str) -> Grant {
            Grant::Deny
        }
    }

    // ------------------------------------------------------------------
    // Tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn declaration_matches_inner_schema_verbatim() {
        let inner: Arc<dyn Tool> = Arc::new(EchoTool);
        let mut p = Policy::new(false, vec![]);
        p.pin("echo", Pin::AlwaysAllow);
        let policy = Arc::new(p);
        let prompter = Arc::new(Mutex::new(AlwaysDeny));
        let adapter = AdkToolAdapter::new(inner.clone(), policy, prompter);

        let decl = adapter.declaration();
        let expected = inner.schema();
        assert_eq!(decl, expected, "declaration must match inner tool's schema verbatim");
    }

    #[tokio::test]
    async fn allowed_call_delegates_and_wraps_result() {
        let inner: Arc<dyn Tool> = Arc::new(EchoTool);
        let mut p = Policy::new(false, vec![]);
        p.pin("echo", Pin::AlwaysAllow);
        let policy = Arc::new(p);
        let prompter = Arc::new(Mutex::new(AlwaysDeny));
        let adapter = AdkToolAdapter::new(inner, policy, prompter);

        let ctx = noop_tool_context();
        let args = json!({"text": "hello"});
        let result = adapter.execute(ctx, args).await.unwrap();
        assert_eq!(result, json!({"result": "echoed: hello"}));
    }

    #[tokio::test]
    async fn denied_call_returns_error_value_not_err() {
        let inner: Arc<dyn Tool> = Arc::new(EchoTool);
        // Non-yolo, no allowlist -> every tool goes through Ask, which the
        // AlwaysDeny prompter rejects.
        let policy = Arc::new(Policy::new(false, vec![]));
        let prompter = Arc::new(Mutex::new(AlwaysDeny));
        let adapter = AdkToolAdapter::new(inner, policy, prompter);

        let ctx = noop_tool_context();
        let args = json!({"text": "secret"});
        let result = adapter.execute(ctx, args).await.unwrap();

        assert!(
            result["error"].as_str().map_or(false, |s| s.to_lowercase().contains("permission denied")),
            "expected error about permission denied, got {result}"
        );
    }
}
