use std::collections::HashMap;
use std::sync::Arc;

use adk_rust::model::openai::{OpenAIClient, OpenAIConfig};
use anyhow::{anyhow, Context};
use entheai_config::ProviderConfig;

/// Resolve a `"<provider>/<model>"` spec (e.g. `"osaurus/qwen3-coder"`) into a
/// live adk-rust model client, using the same `[providers.<name>]` config
/// shape entheai already reads (`base_url` + optional `api_key_env`).
pub fn resolve_model(
    spec: &str,
    providers: &HashMap<String, ProviderConfig>,
) -> anyhow::Result<Arc<dyn adk_rust::Llm>> {
    let (provider_name, model_name) = spec
        .split_once('/')
        .ok_or_else(|| anyhow!("model spec {spec:?} must be \"<provider>/<model>\""))?;
    let pc = providers
        .get(provider_name)
        .ok_or_else(|| anyhow!("unknown provider {provider_name:?} in model spec {spec:?}"))?;
    let api_key = match &pc.api_key_env {
        Some(env_var) => std::env::var(env_var).with_context(|| {
            format!("env var {env_var:?} not set for provider {provider_name:?}")
        })?,
        None => "not-needed".to_string(),
    };
    let config = OpenAIConfig::compatible(&api_key, &pc.base_url, model_name);
    let client = OpenAIClient::new(config)
        .with_context(|| format!("building client for provider {provider_name:?}"))?;
    Ok(Arc::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_provider_slash_model_into_a_client() {
        let mut providers = HashMap::new();
        providers.insert(
            "osaurus".to_string(),
            ProviderConfig {
                base_url: "http://localhost:8000/v1".to_string(),
                api_key_env: None,
            },
        );
        let client = resolve_model("osaurus/qwen3-coder", &providers);
        assert!(
            client.is_ok(),
            "expected a resolved client: {:?}",
            client.err()
        );
    }

    #[test]
    fn unknown_provider_errors() {
        let providers = HashMap::new();
        let client = resolve_model("nope/some-model", &providers);
        assert!(client.is_err());
    }

    #[test]
    fn malformed_spec_without_slash_errors() {
        let providers = HashMap::new();
        let client = resolve_model("no-slash-here", &providers);
        assert!(client.is_err());
    }
}
