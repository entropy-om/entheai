//! Shared per-call plumbing for the entheai-mcp server: config resolution
//! (the CLI's `./entheai.toml → ~/.config/entheai/entheai.toml → built-in`
//! chain, rooted at each tool call's `cwd`), `.env` loading (NATS creds / API
//! keys come from the environment, NEVER from tool args), and the memory-store
//! builder used by the read-only memory tools and the fan-out runtime.
//!
//! Secrets rule: every secret (NATS_URL/NATS_TOKEN, provider API keys) is
//! resolved from the process environment via `dotenvy` — a tool argument is
//! never a place for a credential.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use serde_json::Value;

use entheai_config::Config;

/// Built-in configuration used when neither `cwd/entheai.toml` nor
/// `~/.config/entheai/entheai.toml` exists (mirrors the CLI's fallback:
/// the free, keyless public vaked node). `entheai-config` injects the vaked
/// provider automatically; this just sets the default model.
const BUILTIN_CONFIG_TOML: &str = r#"default_model = "vaked/qwen3-coder:30b"

[providers.vaked]
base_url = "https://coder.vaked.dev/v1"
"#;

/// Default `cwd`-relative config filename (same as the CLI).
const DEFAULT_CONFIG_PATH: &str = "entheai.toml";

/// Expand a leading `~/` to the user's home directory.
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Resolve the per-call working directory: the `cwd` arg when given (must be a
/// directory), else the server's own cwd. Always canonicalized — entheai roots
/// every tool at this path and reads `entheai.toml` there.
pub fn resolve_cwd(args: &Value, server_cwd: &Path) -> anyhow::Result<PathBuf> {
    match args.get("cwd").and_then(Value::as_str) {
        Some(p) if !p.trim().is_empty() => {
            let p = PathBuf::from(p);
            anyhow::ensure!(p.is_dir(), "cwd {p:?} is not a directory");
            p.canonicalize()
                .with_context(|| format!("canonicalizing cwd {p:?}"))
        }
        _ => Ok(server_cwd.to_path_buf()),
    }
}

/// Load `cwd/.env` (non-overriding — an already-set env var wins) so per-repo
/// secrets resolve exactly like the CLI's `dotenvy::dotenv()`. Called at the
/// start of every tool invocation; harmless when no `.env` exists.
pub fn load_env_for(cwd: &Path) {
    let _ = dotenvy::from_path(cwd.join(".env"));
}

/// Load the entheai config rooted at `cwd`, following the CLI's resolution
/// chain: `cwd/entheai.toml` (a present file must parse), then
/// `~/.config/entheai/entheai.toml`, then the built-in default.
pub fn load_config_for(cwd: &Path) -> anyhow::Result<Config> {
    let local = cwd.join(DEFAULT_CONFIG_PATH);
    if local.exists() {
        let text = std::fs::read_to_string(&local)
            .with_context(|| format!("reading config {}", local.display()))?;
        return Ok(Config::from_toml_str(&text)?);
    }

    let global = expand_home("~/.config/entheai/entheai.toml");
    if global.exists() {
        let text = std::fs::read_to_string(&global)
            .with_context(|| format!("reading config {}", global.display()))?;
        log::warn!("no {} in {:?} — using {}", DEFAULT_CONFIG_PATH, cwd, global.display());
        return Ok(Config::from_toml_str(&text)?);
    }

    Ok(Config::from_toml_str(BUILTIN_CONFIG_TOML)?)
}

/// A prompter for unattended (non-interactive) runs: children of an MCP call
/// can never answer a stdin `y/N` prompt, so every agent the bridge builds runs
/// under auto-allow. Mirrors the orchestrator's internal `AutoAllow`.
pub struct AutoAllow;

#[async_trait::async_trait]
impl entheai_permission::Prompter for AutoAllow {
    async fn confirm(&mut self, _tool: &str, _args: &str) -> entheai_permission::Grant {
        entheai_permission::Grant::Allow
    }
}

pub fn auto_allow_prompter() -> Arc<tokio::sync::Mutex<dyn entheai_permission::Prompter>> {
    Arc::new(tokio::sync::Mutex::new(AutoAllow))
}

/// A non-interactive policy: `Mode::Yolo` when `yolo`, else `Mode::Auto` —
/// never `Ask` (a non-interactive child can't answer a stdin prompt).
pub fn unattended_policy(yolo: bool) -> Arc<entheai_permission::Policy> {
    let policy = entheai_permission::Policy::new(yolo, Vec::new());
    policy.set_mode(if yolo {
        entheai_permission::Mode::Yolo
    } else {
        entheai_permission::Mode::Auto
    });
    Arc::new(policy)
}

/// Map the config's `[memory]` block to the runtime config (mirrors
/// bin/entheai's `memory_runtime_config`).
pub fn memory_runtime_config(m: &entheai_config::MemoryConfig) -> entheai_memory::MemoryRuntimeConfig {
    entheai_memory::MemoryRuntimeConfig {
        enabled: m.enabled,
        strict: m.strict,
        retrieve_codebase: m.retrieve_codebase,
        retrieve_learnings: m.retrieve_learnings,
        retrieve_trajectories: m.retrieve_trajectories,
        max_context_chars: m.max_context_chars,
        tool_spill_chars: m.tool_spill_chars,
        evidence_tools: if m.evidence_tools.is_empty() {
            vec!["run_shell".into(), "search".into()]
        } else {
            m.evidence_tools.clone()
        },
    }
}

/// Build the shared memory store from config: an optional embedder (only when
/// `embed_provider` is configured — keeps on-by-default offline-safe) plus the
/// recall params. Returns `None` when memory is disabled.
pub fn build_memory(cfg: &Config) -> anyhow::Result<Option<Arc<dyn entheai_memory::Memory>>> {
    if !cfg.memory.enabled {
        return Ok(None);
    }
    let embedder = cfg.memory.embed_provider.as_ref().and_then(|p| {
        cfg.providers.get(p).map(|pc| {
            entheai_memory::Embedder::new(
                pc.base_url.clone(),
                cfg.memory.embed_model.clone(),
                cfg.memory.embed_timeout_secs,
            )
        })
    });
    let path = expand_home(&cfg.memory.path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut store = entheai_memory::SqliteStore::open(&path, embedder)?;
    store.set_recall_params(entheai_memory::RecallParams {
        w_recency: cfg.memory.w_recency,
        w_conf: cfg.memory.w_conf,
        half_life_days: cfg.memory.half_life_days,
        rrf_k: cfg.memory.rrf_k,
        overfetch: cfg.memory.recall_overfetch,
    });
    Ok(Some(Arc::new(store)))
}

// ── Small arg helpers shared by the tools ────────────────────────────────────

/// Required string arg; errors with a tool-usable message when missing/empty.
pub fn required_str(args: &Value, key: &str) -> anyhow::Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing required argument {key:?}"))
}

/// Optional string arg (None when absent or empty).
pub fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Optional u64 arg.
pub fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

/// Optional bool arg.
pub fn opt_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Unix-epoch milliseconds now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
