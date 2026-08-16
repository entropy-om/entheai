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

/// Process-wide cache of built OpenAI-compatible clients, keyed by
/// `(base_url, api_key, model)`. The TUI rebuilds the agent every turn, so
/// without this each turn re-builds the client (and its underlying reqwest
/// `Client`). Same `Arc<dyn Llm>` shareability argument as `ternary_cache`.
fn openai_cache() -> &'static Mutex<HashMap<String, Arc<dyn adk_rust::Llm>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<dyn adk_rust::Llm>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve a `"<provider>/<model>"` spec (e.g. `"osaurus/qwen3-coder"`) into a
/// live adk-rust model client, using the same `[providers.<name>]` config
/// shape entheai already reads (`base_url` + optional `api_key_env`).
///
/// A provider with `kind = "ternary"` (oracle review #4) bypasses the OpenAI
/// endpoint entirely and builds the native `TernaryLlm` over `model_dir`
/// (ayeOS matrices + embeddings + norms + vendored tokenizer); `kind = "gemini"`
/// builds adk-rust's native `GeminiModel` (see [`resolve_gemini`]).
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
        None | Some("openai") => resolve_openai(provider_name, model_name, pc),
        Some("gemini") => resolve_gemini(provider_name, model_name, pc),
        Some("ternary") => resolve_ternary(provider_name, model_name, pc),
        Some(other) => Err(anyhow!(
            "provider {provider_name:?} has unknown kind {other:?} (expected \"openai\", \"gemini\" or \"ternary\")"
        )),
    }
}

/// Native Gemini API path (`kind = "gemini"`): adk-rust's `GeminiModel` over
/// `generativelanguage.googleapis.com` with the key from `api_key_env`.
/// Preferred over the OpenAI-compatible endpoint for Gemini 3.x, whose
/// tool-call turns require the `thought_signature` round-trip that the
/// OpenAI-compatible client drops (verified live: HTTP 400 "Function call is
/// missing a thought_signature" on the second turn). `base_url` is ignored.
fn resolve_gemini(
    provider_name: &str,
    model_name: &str,
    pc: &ProviderConfig,
) -> anyhow::Result<Arc<dyn adk_rust::Llm>> {
    let env_var = pc.api_key_env.as_deref().ok_or_else(|| {
        anyhow!("provider {provider_name:?} has kind=\"gemini\" but no api_key_env set")
    })?;
    let api_key = std::env::var(env_var).with_context(|| {
        format!(
            "env var {env_var:?} not set for provider {provider_name:?} \
             (export it or add it to .env / ~/.config/entheai/entheai.env)"
        )
    })?;
    let cache_key = format!("gemini\x1f{api_key}\x1f{model_name}");
    if let Some(cached) = openai_cache()
        .lock()
        .expect("openai cache poisoned")
        .get(&cache_key)
    {
        return Ok(Arc::clone(cached));
    }
    let client = adk_rust::model::GeminiModel::new(api_key, model_name)
        .map_err(|e| anyhow!("building Gemini client for provider {provider_name:?}: {e}"))?;
    let llm: Arc<dyn adk_rust::Llm> = Arc::new(client);
    openai_cache()
        .lock()
        .expect("openai cache poisoned")
        .insert(cache_key, Arc::clone(&llm));
    Ok(llm)
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
            format!(
                "env var {env_var:?} not set for provider {provider_name:?} \
                 (export it or add it to .env / ~/.config/entheai/entheai.env; \
                 keyless alternative: --model vaked/qwen3-coder:30b)"
            )
        })?,
        None => "not-needed".to_string(),
    };
    // adk-rust joins `"{base_url}/chat/completions"` verbatim, so a trailing
    // slash (Google documents its OpenAI-compatible base as
    // `.../v1beta/openai/`) yields `//chat/completions` — a 404 on Gemini.
    let base_url = normalize_base_url(&pc.base_url);
    let cache_key = format!("{base_url}\x1f{api_key}\x1f{model_name}");
    if let Some(cached) = openai_cache()
        .lock()
        .expect("openai cache poisoned")
        .get(&cache_key)
    {
        return Ok(Arc::clone(cached));
    }
    let config = OpenAIConfig::compatible(&api_key, base_url, model_name);
    let client = OpenAIClient::new(config)
        .with_context(|| format!("building client for provider {provider_name:?}"))?;
    let llm: Arc<dyn adk_rust::Llm> = Arc::new(client);
    openai_cache()
        .lock()
        .expect("openai cache poisoned")
        .insert(cache_key, Arc::clone(&llm));
    Ok(llm)
}

/// A provider `base_url` without its trailing slash(es), so path joins never
/// produce `//`. Empty stays empty.
fn normalize_base_url(base_url: &str) -> &str {
    base_url.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_url_strips_trailing_slashes_only() {
        assert_eq!(
            normalize_base_url("https://generativelanguage.googleapis.com/v1beta/openai/"),
            "https://generativelanguage.googleapis.com/v1beta/openai"
        );
        assert_eq!(
            normalize_base_url("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(normalize_base_url(""), "");
    }

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
    fn gemini_kind_builds_native_client_from_api_key_env() {
        let mut providers = HashMap::new();
        providers.insert(
            "gemini".to_string(),
            ProviderConfig {
                base_url: String::new(),
                api_key_env: Some("ENTHEAI_TEST_GEMINI_KEY".to_string()),
                model_dir: None,
                kind: Some("gemini".to_string()),
            },
        );
        std::env::remove_var("ENTHEAI_TEST_GEMINI_KEY");
        let err = resolve_model("gemini/gemini-3.6-flash", &providers)
            .err()
            .expect("missing key must error");
        assert!(err.to_string().contains("ENTHEAI_TEST_GEMINI_KEY"), "{err}");

        std::env::set_var("ENTHEAI_TEST_GEMINI_KEY", "test-key");
        let client = resolve_model("gemini/gemini-3.6-flash", &providers);
        assert!(
            client.is_ok(),
            "expected a native Gemini client: {:?}",
            client.err()
        );
        std::env::remove_var("ENTHEAI_TEST_GEMINI_KEY");

        providers.get_mut("gemini").unwrap().api_key_env = None;
        let err = resolve_model("gemini/gemini-3.6-flash", &providers)
            .err()
            .expect("kind=gemini without api_key_env must error");
        assert!(err.to_string().contains("no api_key_env"), "{err}");
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
