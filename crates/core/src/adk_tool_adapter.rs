use std::sync::Arc;

use adk_rust::serde_json::{json, Value};
use adk_rust::{async_trait, Result as AdkResult, Tool as AdkTool, ToolContext};
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
    /// The tool's human description, lifted out of the inner OpenAI-style
    /// schema once (adk's `Tool::description` returns `&str`).
    description: String,
}

impl AdkToolAdapter {
    pub fn new(
        inner: Arc<dyn Tool>,
        policy: Arc<Policy>,
        prompter: Arc<Mutex<dyn Prompter>>,
    ) -> Self {
        let description = function_object(&inner.schema())
            .and_then(|f| f.get("description"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Self {
            inner,
            policy,
            prompter,
            description,
        }
    }
}

/// The `function` object of an OpenAI-style tool schema
/// (`{"type":"function","function":{name,description,parameters}}`), which is
/// how every entheai tool (fs/shell/search/todo/skills/mcp) describes itself.
/// `None` for flat schemas.
fn function_object(schema: &Value) -> Option<&Value> {
    schema.get("function").filter(|f| f.is_object())
}

/// adk-rust reads a tool declaration FLAT — `description` and `parameters` at
/// the top level (adk-model `convert_tools`: `decl.get("description")`,
/// `decl.get("parameters")`, else an empty object schema). Handing it the
/// OpenAI wrapper verbatim therefore ships every tool to the model with no
/// description and no parameters. Unwrap the wrapper; pass flat schemas through.
fn flat_declaration(schema: Value) -> Value {
    match schema.get("function") {
        Some(f) if f.is_object() => f.clone(),
        _ => schema,
    }
}

#[async_trait]
impl AdkTool for AdkToolAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn declaration(&self) -> Value {
        flat_declaration(self.inner.schema())
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
        async_trait, CallbackContext, Content, EventActions, MemoryEntry, ReadonlyContext,
        Result as AdkResult,
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
            serde_json::json!({"type":"function","function":{"name":"echo","description":"Echo the text argument.","parameters":{"type":"object","properties":{"text":{"type":"string"}}}}})
        }
        async fn call(&self, args: serde_json::Value) -> Result<String, entheai_tools::ToolError> {
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

    #[test]
    fn function_object_extracts_the_wrapped_function_and_rejects_non_object() {
        let wrapped = json!({"type":"function","function":{"name":"x","description":"d"}});
        assert_eq!(
            function_object(&wrapped),
            Some(&json!({"name":"x","description":"d"}))
        );

        let flat = json!({"name": "x", "description": "d"});
        assert_eq!(function_object(&flat), None);

        // `function` present but not an object (malformed schema) — still None.
        let malformed = json!({"function": "not an object"});
        assert_eq!(function_object(&malformed), None);
    }

    #[tokio::test]
    async fn declaration_is_flat_name_description_parameters() {
        // adk-model reads `description` / `parameters` at the TOP level of the
        // declaration, so the OpenAI `{"type":"function","function":{..}}`
        // wrapper must be unwrapped — otherwise every tool reaches the model
        // with no description and an empty parameter schema.
        let inner: Arc<dyn Tool> = Arc::new(EchoTool);
        let mut p = Policy::new(false, vec![]);
        p.pin("echo", Pin::AlwaysAllow);
        let policy = Arc::new(p);
        let prompter = Arc::new(Mutex::new(AlwaysDeny));
        let adapter = AdkToolAdapter::new(inner.clone(), policy, prompter);

        let decl = adapter.declaration();
        let inner_fn = inner.schema()["function"].clone();
        assert_eq!(
            decl, inner_fn,
            "declaration must be the unwrapped function object"
        );
        assert_eq!(decl["name"], "echo");
        assert!(
            decl.get("parameters").is_some(),
            "parameters must be top-level"
        );
        assert_eq!(
            adapter.description(),
            inner_fn["description"].as_str().unwrap()
        );

        // Flat (non-wrapped) schemas pass through untouched.
        let flat = json!({"name": "x", "description": "d", "parameters": {"type": "object"}});
        assert_eq!(flat_declaration(flat.clone()), flat);
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
            result["error"]
                .as_str()
                .is_some_and(|s| s.to_lowercase().contains("permission denied")),
            "expected error about permission denied, got {result}"
        );
    }
}
