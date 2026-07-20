use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use entheai_config::Config;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path to the entheai TOML config (same format as the main binary's).
    #[arg(long, default_value = "entheai.toml")]
    config: String,
    /// The sub-agent role (routes to a model via `[agents.<role>]`).
    #[arg(long)]
    role: Option<String>,
    /// The sub-agent's task description.
    #[arg(long)]
    task: Option<String>,
    /// Path to the isolated git worktree this coder should run against.
    #[arg(long)]
    worktree: Option<PathBuf>,
    /// Run as a worker: pull WorkItems from the federation queue and process them.
    #[arg(long)]
    serve: bool,
    /// Dispatch a single coder task to the federation queue and apply the result.
    #[arg(long)]
    dispatch: bool,
    /// For testing: replace the LLM coder with a shell command run in the worktree.
    #[arg(long)]
    test_coder: Option<String>,
}

/// Whether `output` (a coder's captured result text) indicates the sub-agent
/// failed, mirroring `entheai_orchestrator::run_coder_once`'s error-capture
/// convention (`"error: coder failed: {e}"`).
fn is_error_output(output: &str) -> bool {
    output.starts_with("error:")
}

/// Render a coder's result as the single JSON line printed to stdout.
fn render_result(role: &str, task: &str, output: &str) -> String {
    serde_json::json!({ "role": role, "task": task, "output": output }).to_string()
}

/// Worker mode: block on the federation work-queue, materialize each `WorkItem`'s
/// repo from its base bundle, run the coder in an isolated dir, bundle the delta
/// back through the object store, and publish a `WorkResult`. Runs forever.
async fn run_serve(config: &Config, test_coder: Option<&str>) -> anyhow::Result<()> {
    let opts = entheai_federation::FedOptions::from_config(&config.nats, &config.federation);
    let fed = entheai_federation::Federation::connect(&opts).await.ok_or_else(|| {
        anyhow::anyhow!("federation not available (check [federation].enabled + [nats] creds)")
    })?;
    log::info!("entheai-worker: serving the coder work-queue");
    loop {
        let Some(claimed) = fed.claim(std::time::Duration::from_secs(20)).await? else {
            continue;
        };
        let item = claimed.item.clone();
        log::info!("claimed work {}::{} role={}", item.session, item.index, item.role);
        let result = process_one(&fed, config, &item, test_coder).await.unwrap_or_else(|e| {
            entheai_federation::WorkResult {
                session: item.session.clone(),
                index: item.index,
                status: "error".into(),
                committed: false,
                result_bundle_key: String::new(),
                log: format!("error: {e}"),
            }
        });
        fed.publish_result(&result).await.ok();
        claimed.ack().await;
    }
}

/// Process one claimed work item end-to-end: fetch the base bundle, materialize a
/// worktree, run the coder (or a test shell command), and — if the coder changed
/// anything — upload the delta bundle and report `committed`.
async fn process_one(
    fed: &entheai_federation::Federation,
    config: &Config,
    item: &entheai_federation::WorkItem,
    test_coder: Option<&str>,
) -> anyhow::Result<entheai_federation::WorkResult> {
    let tmp = tempfile::tempdir()?;
    let base_bundle = tmp.path().join("base.bundle");
    tokio::fs::write(&base_bundle, fed.get_bundle(&item.base_bundle_key).await?).await?;
    let work = tmp.path().join("work");
    entheai_federation::repo::materialize_from_bundle(&base_bundle, &work).await?;

    // Coder step: real LLM by default; a shell command in test mode.
    let log = match test_coder {
        Some(cmd) => {
            let out = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&work)
                .output()
                .await?;
            format!(
                "test-coder rc={}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout)
            )
        }
        None => entheai_orchestrator::run_coder_once(config, &item.role, &item.task, &work).await,
    };

    let result_bundle = tmp.path().join("result.bundle");
    match entheai_federation::repo::commit_and_bundle_delta(
        &work,
        &item.base_sha,
        &format!("fed: {}", item.task),
        &result_bundle,
    )
    .await?
    {
        Some(_new_sha) => {
            let key = entheai_federation::types::result_key(&item.session, item.index);
            fed.put_bundle(&key, &tokio::fs::read(&result_bundle).await?).await?;
            Ok(entheai_federation::WorkResult {
                session: item.session.clone(),
                index: item.index,
                status: "committed".into(),
                committed: true,
                result_bundle_key: key,
                log: truncate(&log),
            })
        }
        None => Ok(entheai_federation::WorkResult {
            session: item.session.clone(),
            index: item.index,
            status: "no-change".into(),
            committed: false,
            result_bundle_key: String::new(),
            log: truncate(&log),
        }),
    }
}

/// Cap a coder log at a bounded length before it travels over the result subject.
fn truncate(s: &str) -> String {
    s.chars().take(2000).collect()
}

/// Dispatcher mode: bundle the current repo, enqueue a single `WorkItem`, await a
/// worker's `WorkResult`, and apply its delta bundle to a fresh branch. Fail-safe:
/// no result within the deadline means the caller should run locally instead.
async fn run_dispatch(config: &Config, role: &str, task: &str) -> anyhow::Result<()> {
    let opts = entheai_federation::FedOptions::from_config(&config.nats, &config.federation);
    let fed = entheai_federation::Federation::connect(&opts)
        .await
        .ok_or_else(|| anyhow::anyhow!("federation not available"))?;
    let repo = std::env::current_dir()?;
    let session = uuid_like();
    let index = 0usize;

    // Bundle the repo base, upload it.
    let tmp = tempfile::tempdir()?;
    let base_bundle = tmp.path().join("base.bundle");
    let base_sha = entheai_federation::repo::bundle_base(&repo, &base_bundle).await?;
    let base_key = entheai_federation::types::base_key(&session, index);
    fed.put_bundle(&base_key, &tokio::fs::read(&base_bundle).await?).await?;

    // Subscribe BEFORE dispatch so the result isn't missed.
    let mut sub = fed.subscribe_result(&session, index).await?;
    fed.dispatch(&entheai_federation::WorkItem {
        session: session.clone(),
        index,
        role: role.into(),
        task: task.into(),
        base_bundle_key: base_key,
        base_sha: base_sha.clone(),
    })
    .await?;
    println!("dispatched {session}::{index} — awaiting a worker…");

    match fed.await_result(&mut sub).await {
        Some(r) if r.committed => {
            let rb = tmp.path().join("result.bundle");
            tokio::fs::write(&rb, fed.get_bundle(&r.result_bundle_key).await?).await?;
            let branch = format!("fed/{session}-{index}");
            let tip = entheai_federation::repo::apply_delta_bundle(&repo, &rb, &branch).await?;
            println!("worker committed → branch {branch} @ {tip}");
        }
        Some(r) => println!("worker returned status={} (no change applied)\n{}", r.status, r.log),
        None => println!("no worker result within the deadline — dispatch fell through (run locally)."),
    }
    Ok(())
}

/// A per-run identifier for the result subject. Avoids a `uuid` dep — the pid is
/// enough to keep concurrent dispatches on distinct subjects; stays `[a-z0-9]`.
fn uuid_like() -> String {
    format!("d{}", std::process::id())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg_text = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("reading config {}", cli.config))?;
    let config = Config::from_toml_str(&cfg_text)?;

    if cli.serve {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
        return run_serve(&config, cli.test_coder.as_deref()).await;
    }

    if cli.dispatch {
        let role = cli.role.clone().unwrap_or_else(|| "coder".into());
        let task = cli.task.clone().ok_or_else(|| anyhow::anyhow!("--dispatch needs --task"))?;
        return run_dispatch(&config, &role, &task).await;
    }

    // One-shot mode: --role/--task/--worktree are required here.
    let (role, task, worktree) = match (cli.role, cli.task, cli.worktree) {
        (Some(role), Some(task), Some(worktree)) => (role, task, worktree),
        _ => anyhow::bail!(
            "one-shot mode needs --role, --task, and --worktree (or pass --serve / --dispatch)"
        ),
    };

    let output = entheai_orchestrator::run_coder_once(&config, &role, &task, &worktree).await;
    println!("{}", render_result(&role, &task, &output));
    if is_error_output(&output) {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_error_output_detects_the_capture_convention() {
        assert!(is_error_output("error: coder failed: boom"));
        assert!(!is_error_output("added a null check"));
    }

    #[test]
    fn render_result_produces_valid_json_with_expected_fields() {
        let json = render_result("coder", "add x", "did the thing");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["role"], "coder");
        assert_eq!(parsed["task"], "add x");
        assert_eq!(parsed["output"], "did the thing");
    }
}
