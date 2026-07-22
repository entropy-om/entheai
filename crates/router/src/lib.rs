use entheai_config::Config;
use entheai_core::EntheaiAgent;
use entheai_permission::{Policy, Prompter};
use std::sync::Arc;

/// The built-in strong orchestrator when none is configured — the current
/// strongest cheap MoE. Overridable via `[router].orchestrator` / `default_model`.
pub const DEFAULT_ORCHESTRATOR: &str = "deepseek/deepseek-chat";

/// The default orchestrator system prompt (identity + decomposition behavior).
/// Override with `[router].orchestrator_prompt`, extend with `..._append`.
pub const DEFAULT_ORCHESTRATOR_PROMPT: &str = "You are the orchestrator of entheai — a hybrid, fan-out coding agent. You are the strongest model in the swarm; your job is to plan, decompose, and synthesize, not to write code yourself.\n\nGiven a task and repository context you:\n1. Understand the goal and the provided codebase context.\n2. Decompose the work into the smallest set of independent, parallelizable sub-tasks, each matched to a role (explore, coder, test, docs, review). Prefer few well-scoped sub-tasks over many tiny ones, and only decompose when parallelism genuinely helps — a small task is a single sub-task.\n3. Give each sub-agent a precise, self-contained instruction; it sees only its own instruction, not the others'.\n4. After the sub-agents run in isolated git worktrees, synthesize their results into a coherent outcome, resolving conflicts and stating what was done.\n\nPrinciples: correctness first; minimal, focused changes; respect the repository's existing patterns; never fabricate file contents or results; if the task is ambiguous, make the most reasonable assumption and state it. Be decisive and concise.";

/// Orchestrator model id: `[router].orchestrator`, else `default_model`, else
/// the built-in [`DEFAULT_ORCHESTRATOR`].
pub fn orchestrator_model(config: &Config) -> anyhow::Result<String> {
    Ok(config
        .router
        .orchestrator
        .clone()
        .or_else(|| config.default_model.clone())
        .unwrap_or_else(|| DEFAULT_ORCHESTRATOR.to_string()))
}

/// The orchestrator's system prompt: the config override or the built-in
/// default, plus an optional append.
pub fn orchestrator_system_prompt(config: &Config) -> String {
    let mut base = config
        .router
        .orchestrator_prompt
        .clone()
        .unwrap_or_else(|| DEFAULT_ORCHESTRATOR_PROMPT.to_string());
    if let Some(extra) = &config.router.orchestrator_prompt_append {
        base.push_str("\n\n");
        base.push_str(extra);
    }
    base
}

/// Model id for a role: first entry of `[agents.<role>].model`, else the orchestrator/default.
pub fn model_for_role(config: &Config, role: &str) -> anyhow::Result<String> {
    if let Some(a) = config.agents.get(role) {
        if let Some(m) = a.model.first() {
            return Ok(m.clone());
        }
    }
    orchestrator_model(config)
}

/// Build an `EntheaiAgent` for a `"<provider>/<model>"` id using the config's
/// providers and `[inference]` settings. The API key is read from the
/// provider's `api_key_env` at call time (via `EntheaiAgent`'s own model
/// resolution — `provider_name` is validated there, not here).
///
/// `instruction` becomes the agent's system prompt (`LlmAgentBuilder::instruction`),
/// replacing the old pattern of prepending a system `ChatMessage` to every call.
pub fn build_agent(
    model_id: &str,
    config: &Config,
    instruction: Option<&str>,
    registry: entheai_tools::ToolRegistry,
    policy: Arc<Policy>,
    prompter: Arc<tokio::sync::Mutex<dyn Prompter>>,
) -> anyhow::Result<EntheaiAgent> {
    EntheaiAgent::new_with_instruction(
        model_id,
        instruction,
        &config.inference,
        &config.providers,
        registry,
        policy,
        prompter,
        config.router.max_turns as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_router_and_agents() -> Config {
        Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"

            [providers.osaurus]
            base_url = "http://127.0.0.1:1337/v1"

            [providers.zen]
            base_url = "https://opencode.ai/zen/v1"
            api_key_env = "OPENCODE_API_KEY"

            [router]
            orchestrator = "zen/deepseek-v4-pro"
            max_parallel = 4

            [agents.coder]
            model = ["deepseek/deepseek-chat"]
            "#,
        )
        .unwrap()
    }

    #[test]
    fn orchestrator_model_prefers_router_orchestrator() {
        let cfg = cfg_with_router_and_agents();
        assert_eq!(orchestrator_model(&cfg).unwrap(), "zen/deepseek-v4-pro");
    }

    #[test]
    fn orchestrator_model_falls_back_to_default_model() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            "#,
        )
        .unwrap();
        assert_eq!(orchestrator_model(&cfg).unwrap(), "osaurus/qwen3-coder");
    }

    #[test]
    fn orchestrator_model_defaults_to_strong_when_nothing_set() {
        let cfg = Config::from_toml_str("").unwrap();
        assert_eq!(orchestrator_model(&cfg).unwrap(), DEFAULT_ORCHESTRATOR);
        assert_eq!(orchestrator_model(&cfg).unwrap(), "deepseek/deepseek-chat");
    }

    #[test]
    fn orchestrator_system_prompt_default_and_override_and_append() {
        let base = Config::from_toml_str("").unwrap();
        assert_eq!(
            orchestrator_system_prompt(&base),
            DEFAULT_ORCHESTRATOR_PROMPT
        );

        let overridden =
            Config::from_toml_str("[router]\norchestrator_prompt = \"custom brain\"\n").unwrap();
        assert_eq!(orchestrator_system_prompt(&overridden), "custom brain");

        let appended = Config::from_toml_str(
            "[router]\norchestrator_prompt_append = \"Also: prefer Rust.\"\n",
        )
        .unwrap();
        let p = orchestrator_system_prompt(&appended);
        assert!(p.starts_with(DEFAULT_ORCHESTRATOR_PROMPT));
        assert!(p.ends_with("Also: prefer Rust."));
    }

    #[test]
    fn model_for_role_returns_role_specific_model() {
        let cfg = cfg_with_router_and_agents();
        assert_eq!(
            model_for_role(&cfg, "coder").unwrap(),
            "deepseek/deepseek-chat"
        );
    }

    #[test]
    fn model_for_role_falls_back_to_orchestrator_when_role_unset() {
        let cfg = cfg_with_router_and_agents();
        assert_eq!(
            model_for_role(&cfg, "reviewer").unwrap(),
            "zen/deepseek-v4-pro"
        );
    }

    struct AllowAll;
    #[async_trait::async_trait]
    impl Prompter for AllowAll {
        async fn confirm(&mut self, _tool: &str, _args: &str) -> entheai_permission::Grant {
            entheai_permission::Grant::Allow
        }
    }

    fn test_prompter() -> Arc<tokio::sync::Mutex<dyn Prompter>> {
        Arc::new(tokio::sync::Mutex::new(AllowAll))
    }

    #[test]
    fn build_agent_succeeds_for_valid_model_id() {
        let cfg = cfg_with_router_and_agents();
        assert!(build_agent(
            "osaurus/qwen3-coder",
            &cfg,
            None,
            entheai_tools::ToolRegistry::new(),
            Arc::new(Policy::new(true, vec![])),
            test_prompter(),
        )
        .is_ok());
    }

    #[test]
    fn build_agent_errors_on_missing_slash() {
        let cfg = cfg_with_router_and_agents();
        assert!(build_agent(
            "no-slash-here",
            &cfg,
            None,
            entheai_tools::ToolRegistry::new(),
            Arc::new(Policy::new(true, vec![])),
            test_prompter(),
        )
        .is_err());
    }

    #[test]
    fn build_agent_errors_on_unknown_provider() {
        let cfg = cfg_with_router_and_agents();
        assert!(build_agent(
            "nonexistent/some-model",
            &cfg,
            None,
            entheai_tools::ToolRegistry::new(),
            Arc::new(Policy::new(true, vec![])),
            test_prompter(),
        )
        .is_err());
    }
}
