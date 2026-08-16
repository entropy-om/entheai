use entheai_config::Config;
use entheai_core::EntheaiAgent;
use entheai_permission::{Policy, Prompter};
use std::sync::Arc;

/// The built-in strong orchestrator when none is configured — DeepSeek V4 Pro on
/// the direct DeepSeek API (`[providers.deepseek]`, injected by entheai-config
/// so a bare config resolves it as soon as `DEEPSEEK_API_KEY` is set).
/// Overridable via `[router].orchestrator` / `default_model`.
pub const DEFAULT_ORCHESTRATOR: &str = "deepseek/deepseek-v4-pro";

/// The built-in fast/cheap tier — DeepSeek V4 Flash. The default for the light
/// fan-out roles (explore / test / docs) when `[agents.<role>]` is unset, and
/// the interactive default when `default_model` is unset; see
/// [`builtin_model_for_role`].
pub const DEFAULT_FLASH_MODEL: &str = "deepseek/deepseek-v4-flash";

/// Built-in strong-tier fallback chain (most-preferred first): DeepSeek V4 Pro,
/// then Gemini's pro tier, then V4 Pro via OpenRouter. Appended after the
/// operator's own chain for the orchestrator, the coder/reviewer roles and the
/// oracle, so a missing `DEEPSEEK_API_KEY` degrades to Gemini / OpenRouter
/// before the keyless free tier.
pub const DEFAULT_PRO_CHAIN: &[&str] = &[
    DEFAULT_ORCHESTRATOR,
    "gemini/gemini-3.1-pro-preview",
    "openrouter/deepseek/deepseek-v4-pro",
];

/// Built-in flash-tier fallback chain (most-preferred first): DeepSeek V4
/// Flash, then Gemini flash, then V4 Flash via OpenRouter. Appended after the
/// operator's own chain for the light roles (explore / test / docs).
pub const DEFAULT_FLASH_CHAIN: &[&str] = &[
    DEFAULT_FLASH_MODEL,
    "gemini/gemini-3.6-flash",
    "openrouter/deepseek/deepseek-v4-flash",
];

/// The keyless free-tier fallback model — coder.vaked.dev's Qwen3-Coder-30B,
/// via the built-in [`entheai_config::VAKED_PROVIDER`] every config injects.
/// [`available_or_free`] returns this at the fan-out level when no configured
/// provider is actually usable, so a bare `entheai` with zero setup can still
/// fan out instead of erroring on an unresolved provider.
pub const DEFAULT_FREE_MODEL: &str = "vaked/qwen3-coder:30b";

/// The default orchestrator system prompt (identity + decomposition behavior).
/// Override with `[router].orchestrator_prompt`, extend with `..._append`.
pub const DEFAULT_ORCHESTRATOR_PROMPT: &str = "You are the orchestrator of entheai — a hybrid, fan-out coding agent. You are the strongest model in the swarm; your job is to plan, decompose, and synthesize, not to write code yourself.\n\nGiven a task and repository context you:\n1. Understand the goal and the provided codebase context.\n2. Decompose the work into the smallest set of independent, parallelizable sub-tasks, each matched to a role (explore, coder, reviewer, test, docs). Prefer few well-scoped sub-tasks over many tiny ones, and only decompose when parallelism genuinely helps — a small task is a single sub-task.\n3. Give each sub-agent a precise, self-contained instruction; it sees only its own instruction, not the others'.\n4. After the sub-agents run in isolated git worktrees, synthesize their results into a coherent outcome, resolving conflicts and stating what was done.\n\nPrinciples: correctness first; minimal, focused changes; respect the repository's existing patterns; never fabricate file contents or results; if the task is ambiguous, make the most reasonable assumption and state it. Be decisive and concise.";

/// The built-in fallback chain for a fan-out role: the strong tier
/// ([`DEFAULT_PRO_CHAIN`]) for the roles that write or judge code (coder,
/// reviewer), the flash tier ([`DEFAULT_FLASH_CHAIN`]) for the light ones
/// (explore, test, docs, anything else).
pub fn builtin_chain_for_role(role: &str) -> &'static [&'static str] {
    match role.to_ascii_lowercase().as_str() {
        "coder" | "reviewer" | "review" => DEFAULT_PRO_CHAIN,
        _ => DEFAULT_FLASH_CHAIN,
    }
}

/// The built-in model for a fan-out role when nothing is configured for it —
/// the head of [`builtin_chain_for_role`]: DeepSeek V4 Pro for coder/reviewer,
/// V4 Flash for the light roles.
pub fn builtin_model_for_role(role: &str) -> &'static str {
    builtin_chain_for_role(role)[0]
}

/// The operator-configured orchestrator preference chain, most-preferred first:
/// `[router].orchestrator`, then `default_model` (both optional).
fn configured_orchestrator_chain(config: &Config) -> Vec<String> {
    config
        .router
        .orchestrator
        .iter()
        .chain(config.default_model.iter())
        .cloned()
        .collect()
}

/// First model in `candidates` whose provider is [`provider_available`], else the
/// first candidate at all (so the caller's [`available_or_free`] can still
/// degrade it to the free tier, and errors name the intended model rather than
/// a silent substitute). `None` only for an empty list.
fn first_available(config: &Config, candidates: Vec<String>) -> Option<String> {
    candidates
        .iter()
        .find(|m| provider_available(config, m))
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

/// Orchestrator model id: the first *available* of `[router].orchestrator`,
/// `default_model`, then the built-in [`DEFAULT_PRO_CHAIN`] — else the first
/// one configured. Walking on availability is what makes the chain a real
/// fallback: an orchestrator whose key is missing falls through to
/// `default_model`, then Gemini / OpenRouter, instead of straight to the free
/// tier.
pub fn orchestrator_model(config: &Config) -> anyhow::Result<String> {
    let mut chain = configured_orchestrator_chain(config);
    chain.extend(DEFAULT_PRO_CHAIN.iter().map(|m| m.to_string()));
    Ok(first_available(config, chain).expect("chain ends in the built-in pro chain"))
}

/// The Oracle's adjudicator model: `[oracle].model` when its provider is
/// available, else the first available of the built-in [`DEFAULT_PRO_CHAIN`],
/// else `[oracle].model` itself (for the caller's [`available_or_free`]).
pub fn oracle_model(config: &Config) -> String {
    let mut chain = vec![config.oracle.model.clone()];
    chain.extend(DEFAULT_PRO_CHAIN.iter().map(|m| m.to_string()));
    first_available(config, chain).expect("chain is non-empty")
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

/// Model id for a role. `[agents.<role>].model` is a preference-ordered
/// fallback chain, not a label: the first entry whose provider is available
/// (declared + key present) wins. When none is — or the role has no list — the
/// role falls back to the configured orchestrator chain (`[router].orchestrator`,
/// `default_model`), and past that to the role's built-in chain
/// ([`builtin_chain_for_role`]: the DeepSeek V4 Pro tier for coder/reviewer,
/// the V4 Flash tier for the light roles, each with Gemini / OpenRouter
/// fallbacks). Callers on the fan-out path wrap the result in
/// [`available_or_free`] for the final keyless degrade.
pub fn model_for_role(config: &Config, role: &str) -> anyhow::Result<String> {
    let mut chain: Vec<String> = config
        .agents
        .get(role)
        .map(|a| a.model.clone())
        .unwrap_or_default();
    chain.extend(configured_orchestrator_chain(config));
    chain.extend(builtin_chain_for_role(role).iter().map(|m| m.to_string()));
    Ok(first_available(config, chain).expect("chain ends in a built-in tier"))
}

/// Is `model_id`'s provider actually usable right now — known in `[providers]`,
/// and (when it declares an `api_key_env`) that key present in the environment?
/// A keyless provider counts as available as soon as it's declared.
pub fn provider_available(config: &Config, model_id: &str) -> bool {
    let Some((provider, _)) = model_id.split_once('/') else {
        return false;
    };
    match config.providers.get(provider) {
        None => false,
        Some(pc) => match &pc.api_key_env {
            None => true,
            Some(env) => std::env::var(env).is_ok(),
        },
    }
}

/// A model id guaranteed to be buildable: `preferred` when its provider is
/// available, else the keyless free-tier default ([`DEFAULT_FREE_MODEL`]).
///
/// This is the "use coder.vaked.dev's free tier if nothing else is available"
/// rule. The fan-out orchestrator applies it to every leaf and to its own
/// meta-model, so an unconfigured swarm degrades to the free tier instead of
/// erroring on an unresolved provider. Interactive (non-fan-out) use is left
/// untouched — there a misconfiguration should surface loudly, not silently
/// reroute the user's model.
pub fn available_or_free(config: &Config, preferred: String) -> String {
    if provider_available(config, &preferred) {
        return preferred;
    }
    // Loud, not silent: with keyed defaults a missing key would otherwise
    // reroute the whole fan-out to the CPU-slow free tier unnoticed.
    let provider = preferred.split_once('/').map(|(p, _)| p).unwrap_or("?");
    let why = match config.providers.get(provider) {
        None => "not declared in [providers]".to_string(),
        Some(pc) => match &pc.api_key_env {
            Some(env) => format!("env var {env:?} not set"),
            None => "unavailable".to_string(),
        },
    };
    log::warn!(
        "router: {preferred:?} unavailable (provider {provider:?} {why}) — falling back to \
         the keyless free tier {DEFAULT_FREE_MODEL}"
    );
    DEFAULT_FREE_MODEL.to_string()
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
    registry: &entheai_tools::ToolRegistry,
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

    /// Keyless local providers only (`osaurus`, `mlx`) so availability never
    /// depends on the test process's environment; `nowhere/*` is deliberately
    /// undeclared = unavailable.
    fn cfg_with_router_and_agents() -> Config {
        Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"

            [providers.osaurus]
            base_url = "http://127.0.0.1:1337/v1"

            [providers.mlx]
            base_url = "http://127.0.0.1:8080/v1"

            [router]
            orchestrator = "mlx/strong"
            max_parallel = 4

            [agents.coder]
            model = ["nowhere/unavailable", "mlx/coder"]

            [agents.docs]
            model = ["osaurus/qwen3-coder", "mlx/coder"]

            [agents.test]
            model = ["nowhere/a", "nowhere/b"]
            "#,
        )
        .unwrap()
    }

    #[test]
    fn orchestrator_model_prefers_router_orchestrator() {
        let cfg = cfg_with_router_and_agents();
        assert_eq!(orchestrator_model(&cfg).unwrap(), "mlx/strong");
    }

    #[test]
    fn orchestrator_model_skips_unavailable_orchestrator_for_default_model() {
        // The chain is a real fallback: an orchestrator whose provider is
        // unusable falls through to `default_model`, not straight to free tier.
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"

            [providers.osaurus]
            base_url = "http://127.0.0.1:1337/v1"

            [router]
            orchestrator = "nowhere/strong"
            "#,
        )
        .unwrap();
        assert_eq!(orchestrator_model(&cfg).unwrap(), "osaurus/qwen3-coder");
    }

    /// Shadow the injected keyed built-ins with user blocks whose key env is
    /// never set, so "nothing available" holds regardless of the test
    /// process's environment (a dev shell may export the real keys).
    const NO_BUILTIN_KEYS: &str = r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key_env = "ENTHEAI_TEST_KEY_THAT_IS_NEVER_SET"

            [providers.gemini]
            base_url = "https://example.invalid/v1"
            api_key_env = "ENTHEAI_TEST_KEY_THAT_IS_NEVER_SET"

            [providers.openrouter]
            base_url = "https://example.invalid/v1"
            api_key_env = "ENTHEAI_TEST_KEY_THAT_IS_NEVER_SET"
    "#;

    #[test]
    fn orchestrator_model_returns_first_configured_when_nothing_available() {
        // Nothing usable → the intended (first) model, so the caller's
        // `available_or_free` degrades it and errors name the real target.
        let cfg = Config::from_toml_str(&format!(
            r#"
            default_model = "nowhere/b"

            [router]
            orchestrator = "nowhere/a"
            {NO_BUILTIN_KEYS}
            "#
        ))
        .unwrap();
        assert_eq!(orchestrator_model(&cfg).unwrap(), "nowhere/a");
    }

    #[test]
    fn keyed_provider_is_skipped_until_its_env_var_is_set() {
        // The api_key_env axis of availability, end to end through the walk:
        // a declared-but-keyless provider is skipped; once the var exists it wins.
        let cfg = Config::from_toml_str(
            r#"
            [providers.osaurus]
            base_url = "http://127.0.0.1:1337/v1"

            [providers.keyed]
            base_url = "https://example.invalid/v1"
            api_key_env = "ENTHEAI_TEST_KEYED_PROVIDER_KEY"

            [agents.docs]
            model = ["keyed/model", "osaurus/qwen3-coder"]
            "#,
        )
        .unwrap();
        std::env::remove_var("ENTHEAI_TEST_KEYED_PROVIDER_KEY");
        assert!(!provider_available(&cfg, "keyed/model"));
        assert_eq!(model_for_role(&cfg, "docs").unwrap(), "osaurus/qwen3-coder");

        std::env::set_var("ENTHEAI_TEST_KEYED_PROVIDER_KEY", "x");
        assert!(provider_available(&cfg, "keyed/model"));
        assert_eq!(model_for_role(&cfg, "docs").unwrap(), "keyed/model");
        std::env::remove_var("ENTHEAI_TEST_KEYED_PROVIDER_KEY");
    }

    #[test]
    fn builtin_chains_fall_through_to_gemini_then_openrouter() {
        // Bare config, deepseek shadowed as keyless, a keyless user gemini block:
        // orchestrator / coder land on the Gemini pro tier, light roles on the
        // Gemini flash tier — never the free tier while a chain entry works.
        let cfg = Config::from_toml_str(
            r#"
            [providers.deepseek]
            base_url = "https://api.deepseek.com/v1"
            api_key_env = "ENTHEAI_TEST_KEY_THAT_IS_NEVER_SET"

            [providers.gemini]
            base_url = "https://example.invalid/v1"

            [providers.openrouter]
            base_url = "https://example.invalid/v1"
            api_key_env = "ENTHEAI_TEST_KEY_THAT_IS_NEVER_SET"
            "#,
        )
        .unwrap();
        assert_eq!(
            orchestrator_model(&cfg).unwrap(),
            "gemini/gemini-3.1-pro-preview"
        );
        assert_eq!(
            model_for_role(&cfg, "coder").unwrap(),
            "gemini/gemini-3.1-pro-preview"
        );
        assert_eq!(
            model_for_role(&cfg, "explore").unwrap(),
            "gemini/gemini-3.6-flash"
        );
        assert_eq!(oracle_model(&cfg), "gemini/gemini-3.1-pro-preview");
        assert_eq!(DEFAULT_PRO_CHAIN[0], DEFAULT_ORCHESTRATOR);
        assert_eq!(DEFAULT_FLASH_CHAIN[0], DEFAULT_FLASH_MODEL);
    }

    #[test]
    fn orchestrator_model_falls_back_to_default_model() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"

            [providers.osaurus]
            base_url = "http://127.0.0.1:1337/v1"
            "#,
        )
        .unwrap();
        assert_eq!(orchestrator_model(&cfg).unwrap(), "osaurus/qwen3-coder");
    }

    #[test]
    fn orchestrator_model_defaults_to_strong_when_nothing_set() {
        let cfg = Config::from_toml_str(NO_BUILTIN_KEYS).unwrap();
        assert_eq!(orchestrator_model(&cfg).unwrap(), DEFAULT_ORCHESTRATOR);
        assert_eq!(
            orchestrator_model(&cfg).unwrap(),
            "deepseek/deepseek-v4-pro"
        );
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
    fn model_for_role_prefers_first_entry_when_available() {
        let cfg = cfg_with_router_and_agents();
        assert_eq!(model_for_role(&cfg, "docs").unwrap(), "osaurus/qwen3-coder");
    }

    #[test]
    fn model_for_role_walks_list_to_first_available_provider() {
        // `[agents.coder].model[0]` is undeclared → the second entry wins,
        // instead of the old first-only lookup degrading to the free tier.
        let cfg = cfg_with_router_and_agents();
        assert_eq!(model_for_role(&cfg, "coder").unwrap(), "mlx/coder");
    }

    #[test]
    fn model_for_role_falls_back_to_orchestrator_when_role_unset() {
        let cfg = cfg_with_router_and_agents();
        assert_eq!(model_for_role(&cfg, "reviewer").unwrap(), "mlx/strong");
    }

    #[test]
    fn model_for_role_falls_back_to_orchestrator_when_whole_list_unavailable() {
        let cfg = cfg_with_router_and_agents();
        assert_eq!(model_for_role(&cfg, "test").unwrap(), "mlx/strong");
    }

    #[test]
    fn model_for_role_uses_builtin_tier_when_nothing_configured() {
        // Bare config with every keyed built-in shadowed as keyless: nothing is
        // available, so each role reports its intended built-in tier — strong
        // for coder/reviewer, flash for the rest — DeepSeek V4 all the way down.
        let cfg = Config::from_toml_str(NO_BUILTIN_KEYS).unwrap();
        assert_eq!(model_for_role(&cfg, "coder").unwrap(), DEFAULT_ORCHESTRATOR);
        assert_eq!(
            model_for_role(&cfg, "reviewer").unwrap(),
            DEFAULT_ORCHESTRATOR
        );
        assert_eq!(
            model_for_role(&cfg, "explore").unwrap(),
            DEFAULT_FLASH_MODEL
        );
        assert_eq!(model_for_role(&cfg, "test").unwrap(), DEFAULT_FLASH_MODEL);
        assert_eq!(model_for_role(&cfg, "docs").unwrap(), DEFAULT_FLASH_MODEL);
        assert_eq!(orchestrator_model(&cfg).unwrap(), DEFAULT_ORCHESTRATOR);
        assert_eq!(oracle_model(&cfg), cfg.oracle.model);
    }

    #[test]
    fn builtin_tiers_are_deepseek_v4() {
        assert_eq!(builtin_model_for_role("coder"), "deepseek/deepseek-v4-pro");
        assert_eq!(
            builtin_model_for_role("Reviewer"),
            "deepseek/deepseek-v4-pro"
        );
        assert_eq!(
            builtin_model_for_role("explore"),
            "deepseek/deepseek-v4-flash"
        );
        assert_eq!(
            builtin_model_for_role("anything"),
            "deepseek/deepseek-v4-flash"
        );
        // Every built-in chain entry resolves against a provider entheai-config injects.
        let cfg = Config::from_toml_str("").unwrap();
        for m in DEFAULT_PRO_CHAIN.iter().chain(DEFAULT_FLASH_CHAIN.iter()) {
            let (provider, _) = m.split_once('/').unwrap();
            assert!(
                cfg.providers.contains_key(provider),
                "{provider} not injected"
            );
        }
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
            &entheai_tools::ToolRegistry::new(),
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
            &entheai_tools::ToolRegistry::new(),
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
            &entheai_tools::ToolRegistry::new(),
            Arc::new(Policy::new(true, vec![])),
            test_prompter(),
        )
        .is_err());
    }

    #[test]
    fn provider_available_true_for_keyless_declared_provider() {
        let cfg = cfg_with_router_and_agents();
        assert!(provider_available(&cfg, "osaurus/qwen3-coder"));
    }

    #[test]
    fn provider_available_false_for_undeclared_provider() {
        let cfg = cfg_with_router_and_agents();
        assert!(!provider_available(&cfg, "nowhere/unavailable"));
    }

    #[test]
    fn available_or_free_keeps_available_and_falls_back_otherwise() {
        let cfg = cfg_with_router_and_agents();
        // osaurus is declared + keyless → kept as-is.
        assert_eq!(
            available_or_free(&cfg, "osaurus/qwen3-coder".to_string()),
            "osaurus/qwen3-coder"
        );
        // `nowhere` isn't declared → free-tier fallback.
        assert_eq!(
            available_or_free(&cfg, "nowhere/unavailable".to_string()),
            DEFAULT_FREE_MODEL
        );
    }

    #[test]
    fn free_model_resolves_against_the_injected_builtin_provider() {
        // Its provider prefix must match config's injected builtin, else
        // resolve_model would reject it as unknown.
        assert!(DEFAULT_FREE_MODEL.starts_with(&format!("{}/", entheai_config::VAKED_PROVIDER)));
        // And even an empty config carries that provider, keyless, to build it.
        let cfg = Config::from_toml_str("").unwrap();
        assert!(provider_available(&cfg, DEFAULT_FREE_MODEL));
    }
}
