use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use adk_rust::model::openai::{OpenAIClient, OpenAIConfig};
use anyhow::{anyhow, Context};
use entheai_config::ProviderConfig;

use crate::ternary_llm::TernaryLlm;

/// Process-wide cache of loaded ternary models, keyed by canonical `model_dir`.
/// A ternary model + tokenizer is ~400 MiB of reads; the TUI rebuilds the agent
/// every turn, so without this each turn re-loads from disk. `Arc<dyn Llm>` is
/// `Send + Sync` (the `Llm` trait is), so a `Mutex<HashMap>` is enough.
fn ternary_cache() -> &'static Mutex<HashMap<String, Arc<dyn adk_rust::Llm>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<dyn adk_rust::Llm>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve a `"<provider>/<model>"` spec (e.g. `"osaurus/qwen3-coder"`) into a
/// live adk-rust model client, using the same `[providers.<name>]` config
/// shape entheai already reads (`base_url` + optional `api_key_env`).
///
/// A provider with `kind = "ternary"` (oracle review #4) bypasses the OpenAI
/// endpoint entirely and builds the native `TernaryLlm` over `model_dir`
/// (ayeOS matrices + embeddings + norms + vendored tokenizer).
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

    match pc.kind.as_deref() {
        Some("ternary") => resolve_ternary(provider_name, model_name, pc),
        Some(other) => Err(anyhow!(
            "provider {provider_name:?} has unknown kind {other:?} (expected \"openai\" or \"ternary\")"
        )),
        None => resolve_openai(provider_name, model_name, pc),
    }
}

/// Build the native ternary runner from a provider config's `model_dir`.
fn resolve_ternary(
    provider_name: &str,
    model_name: &str,
    pc: &ProviderConfig,
) -> anyhow::Result<Arc<dyn adk_rust::Llm>> {
    let model_dir = pc
        .model_dir
        .as_ref()
        .ok_or_else(|| {
            anyhow!("provider {provider_name:?} has kind=\"ternary\" but no model_dir set")
        })?
        .clone();
    let dir = Path::new(&model_dir);
    anyhow::ensure!(
        dir.is_dir(),
        "provider {provider_name:?} model_dir {} is not a directory",
        dir.display()
    );
    // Canonicalize so the cache key is stable across equivalent path spellings
    // (the TUI rebuilds the agent every turn with the same config string).
    let cache_key = std::fs::canonicalize(dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| model_dir.clone());
    if let Some(cached) = ternary_cache()
        .lock()
        .expect("ternary cache poisoned")
        .get(&cache_key)
    {
        return Ok(Arc::clone(cached));
    }
    let model = ternary::model::TernaryModel::load(dir)
        .with_context(|| format!("loading ternary model from {}", dir.display()))?;
    let tokenizer = ternary::tokenizer::ChatTokenizer::load(dir)
        .with_context(|| format!("loading tokenizer from {}", dir.display()))?;
    let llm: Arc<dyn adk_rust::Llm> =
        Arc::new(TernaryLlm::new(model, tokenizer, model_name.to_string()));
    ternary_cache()
        .lock()
        .expect("ternary cache poisoned")
        .insert(cache_key, Arc::clone(&llm));
    Ok(llm)
}

/// Existing OpenAI-compatible client path (`base_url` + optional `api_key_env`).
fn resolve_openai(
    provider_name: &str,
    model_name: &str,
    pc: &ProviderConfig,
) -> anyhow::Result<Arc<dyn adk_rust::Llm>> {
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

    fn providers_with(
        base_url: &str,
        kind: Option<&str>,
        model_dir: Option<&str>,
    ) -> HashMap<String, ProviderConfig> {
        let mut providers = HashMap::new();
        providers.insert(
            "osaurus".to_string(),
            ProviderConfig {
                base_url: base_url.to_string(),
                api_key_env: None,
                model_dir: model_dir.map(String::from),
                kind: kind.map(String::from),
            },
        );
        providers
    }

    #[test]
    fn resolves_provider_slash_model_into_a_client() {
        let providers = providers_with("http://localhost:8000/v1", None, None);
        let client = resolve_model("osaurus/qwen3-coder", &providers);
        assert!(
            client.is_ok(),
            "expected a resolved client: {:?}",
            client.err()
        );
    }

    #[test]
    fn ternary_kind_without_model_dir_errors() {
        let providers = providers_with("", Some("ternary"), None);
        let err = resolve_model("osaurus/quantal", &providers)
            .err()
            .expect("kind=ternary without model_dir must error");
        assert!(
            err.to_string().contains("no model_dir"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn ternary_kind_with_missing_model_dir_errors() {
        let providers = providers_with("", Some("ternary"), Some("/nonexistent/quantal"));
        let err = resolve_model("osaurus/quantal", &providers)
            .err()
            .expect("kind=ternary with missing model_dir must error");
        assert!(
            err.to_string().contains("is not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unknown_kind_errors() {
        let providers = providers_with("", Some("banana"), None);
        let err = resolve_model("osaurus/x", &providers)
            .err()
            .expect("unknown kind must error");
        assert!(
            err.to_string().contains("unknown kind"),
            "unexpected error: {err}"
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

    /// Real-model happy path: `kind = "ternary"` + `model_dir` resolves to a
    /// live `TernaryLlm` whose name is the model half of the spec.
    ///
    /// Uses the same `AYEOS_DATA_DIR` convention as `crates/ternary` (else the
    /// workspace-relative quantal dir); skips gracefully when the model dir is
    /// not present so the core suite stays green without the ~400 MiB model.
    fn data_dir() -> std::path::PathBuf {
        if let Ok(dir) = std::env::var("AYEOS_DATA_DIR") {
            return std::path::PathBuf::from(dir);
        }
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../pocoo.vaked.dev/demos/quantal")
    }

    #[test]
    fn ternary_kind_with_model_dir_resolves_ternary_llm() {
        let dir = data_dir();
        if !dir.is_dir() {
            eprintln!("skipping: model dir {dir:?} not present (set AYEOS_DATA_DIR)");
            return;
        }
        let mut providers = HashMap::new();
        providers.insert(
            "quantal".to_string(),
            ProviderConfig {
                base_url: "unused".to_string(),
                api_key_env: None,
                model_dir: Some(dir.display().to_string()),
                kind: Some("ternary".to_string()),
            },
        );
        let llm = resolve_model("quantal/quantal", &providers)
            .unwrap_or_else(|e| panic!("resolve failed: {e}"));
        assert_eq!(
            llm.name(),
            "quantal",
            "ternary Llm name must be the model half of the spec"
        );
    }
}
