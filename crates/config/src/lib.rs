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
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
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
}
