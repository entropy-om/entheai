//! `entheai_run`: one-shot model call via the engine's own agent path
//! (`EntheaiAgent::build_auto` + `run_to_text`), which resolves the model
//! through `entheai_core::model_resolve::resolve_model` — so
//! `model: "quantal/quantal"` routes to the offline native ternary runner.
//! Non-interactive: built with an empty tool registry and an auto-allow
//! prompter, so it can never block on a stdin permission prompt.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::engine::{
    auto_allow_prompter, load_config_for, load_env_for, resolve_cwd, unattended_policy,
};

/// `{prompt, cwd?, model?, yolo?, timeout_secs?} → {answer, model, duration_ms}`.
pub async fn entheai_run(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let prompt = crate::engine::required_str(&args, "prompt")?;
    let cwd = resolve_cwd(&args, &server_cwd)?;
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;

    let model = crate::engine::opt_str(&args, "model")
        .or_else(|| cfg.default_model.clone())
        .unwrap_or_else(|| entheai_router::DEFAULT_ORCHESTRATOR.to_string());
    let yolo = crate::engine::opt_bool(&args, "yolo");
    let timeout_secs = crate::engine::opt_u64(&args, "timeout_secs").unwrap_or(300);
    let max_iterations: u32 = if yolo {
        u32::MAX
    } else {
        cfg.router.max_turns as u32
    };

    let registry = entheai_tools::ToolRegistry::new();
    let scope = entheai_memory::MemoryScope {
        session_id: uuid::Uuid::new_v4().simple().to_string(),
        task_id: "run".to_string(),
        cwd: cwd.clone(),
        role: None,
    };
    let agent = entheai_core::EntheaiAgent::build_auto(
        &model,
        None,
        &cfg.inference,
        &cfg.providers,
        &registry,
        unattended_policy(yolo),
        auto_allow_prompter(),
        max_iterations,
        None,
        None,
        scope,
        None,
    )?;

    let t0 = Instant::now();
    let answer = tokio::time::timeout(Duration::from_secs(timeout_secs), agent.run_to_text(&prompt))
        .await
        .map_err(|_| anyhow::anyhow!("entheai_run timed out after {timeout_secs}s"))??;

    Ok(json!({
        "answer": answer,
        "model": model,
        "duration_ms": t0.elapsed().as_millis(),
        "executed_on": "local",
    }))
}
