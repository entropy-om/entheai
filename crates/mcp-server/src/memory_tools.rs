//! `entheai_memory_search` / `entheai_memory_stats` / `entheai_memory_list`:
//! read-only access to the engine's 5-namespace SQLite memory. Writes stay with
//! entheai — these tools only ask the brain. Keyword-only search when no
//! embedder is configured (offline-safe), hybrid vector+keyword otherwise.

use std::path::PathBuf;

use serde_json::{json, Value};

use entheai_memory::Namespace;

use crate::engine::{build_memory, load_config_for, load_env_for, opt_u64, resolve_cwd};

/// Preview cap per entry — enough context for the caller, bounded for the wire.
const PREVIEW_CHARS: usize = 4000;

/// `{namespace, query, limit?, cwd?} → {namespace, query, results: [{score, key, content, created_at}]}`.
pub async fn entheai_memory_search(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let cwd = resolve_cwd(&args, &server_cwd)?;
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;

    let ns = parse_ns(crate::engine::required_str(&args, "namespace")?)?;
    let query = crate::engine::required_str(&args, "query")?;
    let limit = opt_u64(&args, "limit").unwrap_or(10).min(50) as usize;

    let Some(store) = build_memory(&cfg)? else {
        anyhow::bail!("memory is disabled ([memory] enabled = false)");
    };
    let hits = store.search(ns, &query, limit).await?;
    let results: Vec<Value> = hits
        .iter()
        .map(|s| {
            json!({
                "score": s.score,
                "key": s.entry.key,
                "content": preview(&s.entry.content),
                "created_at": s.entry.created_at,
                "metadata": s.entry.metadata,
            })
        })
        .collect();
    Ok(json!({
        "namespace": ns.as_str(),
        "query": query,
        "count": results.len(),
        "results": results,
    }))
}

/// `{} → {codebase, learnings, trajectories, tools, subagents, total}`.
pub async fn entheai_memory_stats(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let cwd = resolve_cwd(&args, &server_cwd)?;
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;

    let Some(store) = build_memory(&cfg)? else {
        anyhow::bail!("memory is disabled ([memory] enabled = false)");
    };
    let mut out = serde_json::Map::new();
    let mut total = 0usize;
    for ns in [
        Namespace::Codebase,
        Namespace::Learnings,
        Namespace::Trajectories,
        Namespace::Tools,
        Namespace::Subagents,
    ] {
        let n = store.count(ns).await?;
        total += n;
        out.insert(ns.as_str().to_string(), json!(n));
    }
    out.insert("total".to_string(), json!(total));
    Ok(Value::Object(out))
}

/// `{namespace, limit?, cwd?} → {namespace, entries: [{key, content, created_at, updated_at}]}`.
pub async fn entheai_memory_list(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let cwd = resolve_cwd(&args, &server_cwd)?;
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;

    let ns = parse_ns(crate::engine::required_str(&args, "namespace")?)?;
    let limit = opt_u64(&args, "limit").unwrap_or(10).min(50) as usize;

    let Some(store) = build_memory(&cfg)? else {
        anyhow::bail!("memory is disabled ([memory] enabled = false)");
    };
    let entries = store.list(ns, limit, 0).await?;
    let list: Vec<Value> = entries
        .iter()
        .map(|e| {
            json!({
                "key": e.key,
                "content": preview(&e.content),
                "created_at": e.created_at,
                "updated_at": e.updated_at,
                "metadata": e.metadata,
            })
        })
        .collect();
    Ok(json!({"namespace": ns.as_str(), "count": list.len(), "entries": list}))
}

fn parse_ns(s: String) -> anyhow::Result<Namespace> {
    s.parse::<Namespace>().map_err(|_| {
        anyhow::anyhow!("unknown namespace '{s}' (codebase|learnings|trajectories|tools|subagents)")
    })
}

fn preview(s: &str) -> String {
    if s.chars().count() <= PREVIEW_CHARS {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(PREVIEW_CHARS).collect();
        out.push_str("\n…[truncated]");
        out
    }
}
