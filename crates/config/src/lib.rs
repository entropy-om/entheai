use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse config TOML: {0}")]
    Toml(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub companion: CompanionConfig,
    #[serde(default)]
    pub router: RouterConfig,
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouterConfig {
    /// Model id ("<provider>/<model>") for the orchestrator role. Falls back
    /// to `default_model` when unset.
    #[serde(default)]
    pub orchestrator: Option<String>,
    /// Max number of sub-agents that may run concurrently during fan-out.
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            orchestrator: None,
            max_parallel: default_max_parallel(),
        }
    }
}

fn default_max_parallel() -> usize {
    8
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentConfig {
    /// Preference-ordered model ids ("<provider>/<model>") for this role.
    #[serde(default)]
    pub model: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompanionConfig {
    /// Whether to spawn the companion window. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether the companion floats above other windows. Default: true.
    #[serde(default = "default_true")]
    pub always_on_top: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CompanionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            always_on_top: true,
        }
    }
}

impl Config {
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_providers_and_default_model() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"

            [providers.osaurus]
            base_url = "http://127.0.0.1:1337/v1"

            [providers.zen]
            base_url = "https://opencode.ai/zen/v1"
            api_key_env = "OPENCODE_API_KEY"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.default_model.as_deref(), Some("osaurus/qwen3-coder"));
        assert_eq!(
            cfg.providers["osaurus"].base_url,
            "http://127.0.0.1:1337/v1"
        );
        assert_eq!(cfg.providers["osaurus"].api_key_env, None);
        assert_eq!(
            cfg.providers["zen"].api_key_env.as_deref(),
            Some("OPENCODE_API_KEY")
        );
    }

    #[test]
    fn parses_router_and_agents_when_present() {
        let cfg = Config::from_toml_str(
            r#"
            [router]
            orchestrator = "zen/deepseek-v4-pro"
            max_parallel = 4

            [agents.coder]
            model = ["deepseek/deepseek-chat"]
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.router.orchestrator.as_deref(),
            Some("zen/deepseek-v4-pro")
        );
        assert_eq!(cfg.router.max_parallel, 4);
        assert_eq!(
            cfg.agents["coder"].model,
            vec!["deepseek/deepseek-chat".to_string()]
        );
    }

    #[test]
    fn router_and_agents_default_when_absent() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.router.orchestrator, None);
        assert_eq!(cfg.router.max_parallel, 8);
        assert!(cfg.agents.is_empty());
    }
}
