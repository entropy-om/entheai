use entheai_config::Config;
use entheai_core::Agent;
use entheai_providers::OpenAiCompatProvider;

/// Orchestrator model id: `[router].orchestrator`, else `default_model`.
pub fn orchestrator_model(config: &Config) -> anyhow::Result<String> {
    config
        .router
        .orchestrator
        .clone()
        .or_else(|| config.default_model.clone())
        .ok_or_else(|| {
            anyhow::anyhow!("no orchestrator: set [router].orchestrator or default_model")
        })
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

/// Build an `Agent` for a `"<provider>/<model>"` id using the config's providers.
/// The API key is read from the provider's `api_key_env` at call time.
pub fn build_agent(model_id: &str, config: &Config) -> anyhow::Result<Agent<OpenAiCompatProvider>> {
    let (provider_name, model) = model_id
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("model must be '<provider>/<model>': {model_id}"))?;
    let pcfg = config
        .providers
        .get(provider_name)
        .ok_or_else(|| anyhow::anyhow!("unknown provider '{provider_name}'"))?;
    let api_key = pcfg
        .api_key_env
        .as_ref()
        .and_then(|e| std::env::var(e).ok());
    let provider = OpenAiCompatProvider::new(pcfg.base_url.clone(), api_key);
    Ok(Agent::new(provider, model.to_string()))
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
    fn orchestrator_model_errors_when_nothing_set() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(orchestrator_model(&cfg).is_err());
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

    #[test]
    fn build_agent_succeeds_for_valid_model_id() {
        let cfg = cfg_with_router_and_agents();
        assert!(build_agent("osaurus/qwen3-coder", &cfg).is_ok());
    }

    #[test]
    fn build_agent_errors_on_missing_slash() {
        let cfg = cfg_with_router_and_agents();
        assert!(build_agent("no-slash-here", &cfg).is_err());
    }

    #[test]
    fn build_agent_errors_on_unknown_provider() {
        let cfg = cfg_with_router_and_agents();
        assert!(build_agent("nonexistent/some-model", &cfg).is_err());
    }
}
