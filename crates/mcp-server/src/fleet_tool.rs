//! `entheai_fleet_status`: the live worker roster from the NATS presence
//! heartbeat. `Federation::list_workers` ping/collects the `entheai.presence.*`
//! subjects and dedups to one entry per node — this is exactly the data behind
//! the engine's `/fleet` UI, exposed as a tool.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use entheai_federation::{FedOptions, Federation};

use crate::engine::{load_config_for, load_env_for, resolve_cwd};

/// `{} → {workers: [{node_id, hostname, state, started_at_unix}], count, available}`.
pub async fn entheai_fleet_status(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let cwd = resolve_cwd(&args, &server_cwd)?;
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;

    let opts = FedOptions::from_config(&cfg.nats, &cfg.federation);
    let Some(fed) = Federation::connect(&opts).await else {
        return Ok(json!({
            "workers": [],
            "count": 0,
            "available": false,
            "message": "federation is not available (check [federation].enabled + [nats] creds)",
        }));
    };

    // Collect presence for a short window after pinging; 2s keeps the tool snappy.
    let workers = fed.list_workers(Duration::from_secs(2)).await;
    let roster: Vec<Value> = workers
        .iter()
        .map(|w| {
            json!({
                "node_id": w.node_id,
                "hostname": w.hostname,
                "version": w.version,
                "state": w.state,
                "started_at_unix": w.started_at_unix,
            })
        })
        .collect();
    Ok(json!({
        "workers": roster,
        "count": roster.len(),
        "available": !roster.is_empty(),
    }))
}
