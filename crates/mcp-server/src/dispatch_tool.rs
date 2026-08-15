//! `entheai_dispatch`: a single-task fleet round-trip over NATS, mirroring
//! `entheai-worker --dispatch` but AS A LIBRARY (no shell-out). Subscribe to the
//! result subject BEFORE dispatching, await the worker's `WorkResult`, and —
//! when it committed — apply its delta bundle to `fed/<session>-<index>`.
//!
//! Honesty contract (oracle review): with no worker, on error, or past the
//! deadline, the result says `executed_on: "local"` / `status:
//! "fell_through_local"` — the caller must run the task itself, never pretend
//! the fleet did it.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use serde_json::{json, Value};

use entheai_federation::{FedOptions, Federation, WorkItem};

use crate::engine::{load_config_for, load_env_for, opt_str, resolve_cwd};

/// `{task, cwd?, role?, deadline_secs?} → {status, branch?, log?, base?, executed_on}`.
pub async fn entheai_dispatch(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let task = crate::engine::required_str(&args, "task")?;
    let cwd = resolve_cwd(&args, &server_cwd)?;
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;

    let role = opt_str(&args, "role").unwrap_or_else(|| "coder".to_string());
    let deadline_secs =
        crate::engine::opt_u64(&args, "deadline_secs").unwrap_or(cfg.federation.deadline_secs);

    let opts = FedOptions::from_config(&cfg.nats, &cfg.federation);
    let Some(fed) = Federation::connect(&opts).await else {
        // Federation off or the hub unreachable → the fleet never ran this task.
        return Ok(json!({
            "status": "fell_through_local",
            "executed_on": "local",
            "message": "federation is not available (check [federation].enabled + [nats] creds)",
        }));
    };

    let repo = cwd.clone();
    anyhow::ensure!(
        entheai_federation::repo::rev_parse_ok(&repo, "HEAD").await,
        "dispatch needs a git repo at {repo:?} (no resolvable HEAD)"
    );

    let session = uuid::Uuid::new_v4().simple().to_string();
    let index = 0usize;

    // Bundle the repo base + upload it, then subscribe BEFORE dispatching so
    // the core-NATS result can't slip past us.
    let tmp = tempfile::tempdir().context("creating temp dir for dispatch bundles")?;
    let base_bundle = tmp.path().join("base.bundle");
    let base_sha = entheai_federation::repo::bundle_base(&repo, &base_bundle).await?;
    let base_key = entheai_federation::types::base_key(&session, index);
    fed.put_bundle(&base_key, &tokio::fs::read(&base_bundle).await?)
        .await?;

    let mut sub = fed.subscribe_result(&session, index).await?;
    fed.dispatch(&WorkItem {
        session: session.clone(),
        index,
        role: role.clone(),
        task: task.clone(),
        base_bundle_key: base_key,
        base_sha: base_sha.clone(),
    })
    .await?;

    let deadline = Duration::from_secs(deadline_secs);
    let awaited = tokio::time::timeout(deadline, sub.next())
        .await
        .map_err(|_| {
            anyhow::anyhow!("no worker result within {deadline_secs}s — fell through (run locally)")
        })?;
    let r: Option<entheai_federation::WorkResult> = match awaited {
        Some(msg) => serde_json::from_slice(&msg.payload).ok(),
        _ => None,
    };

    let Some(r) = r else {
        return Ok(json!({
            "status": "fell_through_local",
            "executed_on": "local",
            "message": format!("no worker result for session {session} within {deadline_secs}s"),
        }));
    };

    if r.committed {
        let rb = tmp.path().join("result.bundle");
        tokio::fs::write(&rb, fed.get_bundle(&r.result_bundle_key).await?)
            .await
            .context("writing the worker's delta bundle")?;
        let branch = format!("fed/{session}-{index}");
        let tip = entheai_federation::repo::apply_delta_bundle(&repo, &rb, &branch).await?;
        Ok(json!({
            "status": "committed",
            "branch": branch,
            "tip": tip,
            "log": r.log,
            "base": r.base,
            "executed_on": "fleet",
        }))
    } else {
        // no-change / error from the worker: nothing applied, honest about where it ran.
        Ok(json!({
            "status": r.status,
            "committed": false,
            "log": r.log,
            "base": r.base,
            "executed_on": "fleet",
        }))
    }
}
