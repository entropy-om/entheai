//! `entheai_fanout` + `entheai_job_status`: the flagship bridge tool. A fan-out
//! is LONG, so the tool call returns a `job_id` immediately and the work runs in
//! a spawned tokio task, persisting state to `~/.cache/entheai-bridge/jobs/<id>.json`
//! at every phase (queued → running → done/error) so `entheai_job_status` can
//! poll it and the state survives the MCP call.
//!
//! The job calls `entheai_orchestrator::run_fanout_detailed` AS A LIBRARY — the
//! outcome carries the full `MergeSeal` per verified coder (`diff_sha256`,
//! `log_sha256`, `seal`), which no CLI exposes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use entheai_federation::{FedOptions, Federation, FederationExecutor};
use entheai_memory::{MemoryRuntime, MemoryScope};
use entheai_orchestrator::{CoderOutcome, VerifyStatus, WorkerPool};

use crate::engine::{
    build_memory, load_config_for, load_env_for, memory_runtime_config, now_ms, opt_bool, opt_str,
    opt_u64, resolve_cwd,
};

/// Echo of the fan-out request, persisted with the job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRequest {
    pub task: String,
    pub cwd: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub verify: Option<String>,
    #[serde(default)]
    pub max_parallel: Option<u64>,
    #[serde(default)]
    pub yolo: bool,
    #[serde(default)]
    pub deadline_minutes: Option<u64>,
}

/// Lifecycle of a persisted job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Error,
}

/// One on-disk job record (`~/.cache/entheai-bridge/jobs/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub status: JobStatus,
    pub progress: Option<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub request: JobRequest,
    pub result: Option<Value>,
}

/// `~/.cache/entheai-bridge/jobs`
pub fn jobs_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
        .join(".cache")
        .join("entheai-bridge")
        .join("jobs")
}

pub fn job_path(id: &str) -> PathBuf {
    jobs_dir().join(format!("{id}.json"))
}

fn write_job(rec: &JobRecord) -> anyhow::Result<()> {
    std::fs::create_dir_all(jobs_dir())?;
    std::fs::write(job_path(&rec.id), serde_json::to_string_pretty(rec)?)?;
    Ok(())
}

/// `{task, cwd, model?, verify?, max_parallel?, yolo?, deadline_minutes?} → {job_id}`.
/// Spawns the fan-out in a background tokio task and returns immediately.
pub async fn entheai_fanout(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let task = crate::engine::required_str(&args, "task")?;
    let cwd = resolve_cwd(&args, &server_cwd)?;

    let request = JobRequest {
        task,
        cwd: cwd.display().to_string(),
        model: opt_str(&args, "model"),
        verify: opt_str(&args, "verify"),
        max_parallel: opt_u64(&args, "max_parallel"),
        yolo: opt_bool(&args, "yolo"),
        deadline_minutes: opt_u64(&args, "deadline_minutes"),
    };

    let id = uuid::Uuid::new_v4().simple().to_string();
    let rec = JobRecord {
        id: id.clone(),
        status: JobStatus::Queued,
        progress: None,
        created_at_unix_ms: now_ms(),
        updated_at_unix_ms: now_ms(),
        request: request.clone(),
        result: None,
    };
    // Written synchronously so `entheai_job_status` can find the job the moment
    // this tool call returns, before the spawned task even runs.
    write_job(&rec)?;

    tokio::spawn(run_fanout_job(id.clone(), request));
    Ok(json!({"job_id": id}))
}

/// `{job_id} → {status, progress, result?, request, created_at_unix_ms, ...}`.
pub async fn entheai_job_status(args: Value, _server_cwd: PathBuf) -> anyhow::Result<Value> {
    let job_id = crate::engine::required_str(&args, "job_id")?;
    // Defense: a hostile/accidental job_id must not escape the jobs dir.
    anyhow::ensure!(
        job_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "invalid job_id {job_id:?}"
    );
    let path = job_path(&job_id);
    if !path.exists() {
        return Ok(json!({"job_id": job_id, "status": "not_found"}));
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let rec: JobRecord = serde_json::from_str(&text)?;
    Ok(serde_json::to_value(rec)?)
}

/// The background job body: run the fan-out, persist the structured result.
async fn run_fanout_job(id: String, req: JobRequest) {
    // Read back the queued record (written synchronously by the tool call) so
    // `created_at` survives; fall back to a fresh record if it's somehow gone.
    let mut rec: JobRecord = std::fs::read_to_string(job_path(&id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| JobRecord {
            id: id.clone(),
            status: JobStatus::Queued,
            progress: None,
            created_at_unix_ms: now_ms(),
            updated_at_unix_ms: now_ms(),
            request: req.clone(),
            result: None,
        });
    rec.status = JobStatus::Running;
    rec.progress = Some("decomposing + dispatching coders…".to_string());
    rec.updated_at_unix_ms = now_ms();
    if write_job(&rec).is_err() {
        log::warn!("entheai-mcp: could not persist running state for job {id}");
    }

    let outcome = run_fanout_core(&req).await;

    match outcome {
        Ok(result) => {
            rec.status = JobStatus::Done;
            rec.progress = None;
            rec.result = Some(result);
            rec.updated_at_unix_ms = now_ms();
            let _ = write_job(&rec);
        }
        Err(e) => {
            log::warn!("entheai-mcp: job {id} failed: {e:#}");
            rec.status = JobStatus::Error;
            rec.progress = Some(format!("error: {e:#}"));
            rec.result = None;
            rec.updated_at_unix_ms = now_ms();
            let _ = write_job(&rec);
        }
    }
}

/// Execute one fan-out run and render the structured result (report + session +
/// base + per-coder outcomes with full MergeSeal). All config/env resolution
/// happens against the request's `cwd`, exactly like a CLI run from that repo.
async fn run_fanout_core(req: &JobRequest) -> anyhow::Result<Value> {
    let cwd = PathBuf::from(&req.cwd);
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;
    let root = cwd
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", cwd.display()))?;

    // Per-call overrides on a config clone (never mutates the shared config).
    let mut cfg = cfg;
    if let Some(verify) = req.verify.as_deref().filter(|v| !v.is_empty()) {
        cfg.fanout.verify = Some(verify.to_string());
    }
    if let Some(mp) = req.max_parallel.filter(|m| *m > 0) {
        cfg.router.max_parallel = mp as usize;
    }
    // Non-interactive children can't answer stdin prompts: force the fan-out
    // policy to a non-Ask mode. Explicit yolo wins; otherwise the default Auto
    // ceiling applies (never Plan/Ask).
    if req.yolo {
        cfg.fanout.mode = "yolo".to_string();
    } else if cfg.fanout.mode.is_empty() {
        cfg.fanout.mode = "auto".to_string();
    }

    // Executor selection mirrors bin/entheai's `--fanout` path: agy/copilot
    // executors when configured, else federation when enabled, else local.
    let fed_exec: Option<Arc<dyn entheai_orchestrator::CoderExecutor>> =
        if cfg.fanout.executor == "agy" {
            Some(
                entheai_orchestrator::AgyExecutor::new(cfg.fanout.agy_model.clone())
                    as Arc<dyn entheai_orchestrator::CoderExecutor>,
            )
        } else if cfg.fanout.executor == "copilot" {
            Some(
                entheai_orchestrator::CopilotExecutor::new(cfg.fanout.copilot_model.clone())
                    as Arc<dyn entheai_orchestrator::CoderExecutor>,
            )
        } else if cfg.federation.enabled {
            Federation::connect(&FedOptions::from_config(&cfg.nats, &cfg.federation))
                .await
                .map(|f| {
                    FederationExecutor::new(f, root.clone())
                        as Arc<dyn entheai_orchestrator::CoderExecutor>
                })
        } else {
            None
        };

    let pool = WorkerPool::new(cfg.router.max_parallel.max(1));
    let memory = build_memory(&cfg)?;
    let runtime =
        memory.map(|m| Arc::new(MemoryRuntime::new(m, memory_runtime_config(&cfg.memory))));
    let scope = MemoryScope {
        session_id: uuid::Uuid::new_v4().simple().to_string(),
        task_id: "fanout".to_string(),
        cwd: root.clone(),
        role: None,
    };

    let run = match req.deadline_minutes {
        Some(min) if min > 0 => tokio::time::timeout(
            Duration::from_secs(min * 60),
            entheai_orchestrator::run_fanout_detailed(
                &cfg,
                &root,
                &req.task,
                None,
                pool,
                fed_exec,
                entheai_orchestrator::oracle_for_config(&cfg),
                runtime,
                scope,
                None,
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("fan-out job exceeded the {min}-minute deadline"))??,
        _ => {
            entheai_orchestrator::run_fanout_detailed(
                &cfg,
                &root,
                &req.task,
                None,
                pool,
                fed_exec,
                entheai_orchestrator::oracle_for_config(&cfg),
                runtime,
                scope,
                None,
            )
            .await?
        }
    };

    let outcomes: Vec<Value> = run.outcomes.iter().map(outcome_to_json).collect();
    Ok(json!({
        "report": run.report,
        "session": run.session,
        "base": run.base,
        "outcomes": outcomes,
    }))
}

/// Serialize one coder outcome — the full `MergeSeal` for `VerifyStatus::Passed`
/// is what makes the library call worthwhile (no CLI exposes it).
fn outcome_to_json(o: &CoderOutcome) -> Value {
    let verify = match &o.verify {
        VerifyStatus::Passed(seal) => json!({
            "kind": "passed",
            "diff_sha256": seal.diff_sha256,
            "log_sha256": seal.log_sha256,
            "seal": seal.seal,
            "verify_cmd": seal.verify_cmd,
        }),
        VerifyStatus::NoChanges => json!({"kind": "no_changes"}),
        VerifyStatus::Skipped => json!({"kind": "skipped"}),
        VerifyStatus::Unverifiable => json!({"kind": "unverifiable"}),
        VerifyStatus::Failed(log) => json!({"kind": "failed", "log": log}),
    };
    json!({
        "index": o.index,
        "role": o.role,
        "task": o.task,
        "branch": o.branch,
        "committed": o.committed,
        "integrated": o.integrated,
        "conflicted": o.conflicted,
        "timed_out": o.timed_out,
        "oracle_rejected": o.oracle_rejected,
        "verify": verify,
        "output": cap_chars(&o.output, 20_000),
    })
}

/// Cap a coder's raw output so a huge shell dump can't bloat the job file/poll.
fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push_str("\n…[truncated]");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_status_rejects_traversal_ids() {
        // job_id must stay a safe filename inside the jobs dir.
        assert!(entheai_job_status_safe("../../etc/passwd").is_err());
        assert!(entheai_job_status_safe("abc123-_").is_ok());
        assert!(entheai_job_status_safe("a b").is_err());
    }

    /// Standalone validation (the async tool needs a runtime; this tests only
    /// the filename guard, which is sync).
    fn entheai_job_status_safe(job_id: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            job_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "invalid job_id {job_id:?}"
        );
        Ok(())
    }

    #[test]
    fn outcome_json_carries_the_full_merge_seal() {
        let seal = entheai_orchestrator::MergeSeal::compute(b"diff", b"log", "./scripts/check.sh");
        let o = CoderOutcome {
            index: 0,
            role: "coder".into(),
            task: "add a test".into(),
            branch: "entheai/abc/coder-0".into(),
            output: "did it".into(),
            committed: true,
            verify: VerifyStatus::Passed(seal.clone()),
            integrated: true,
            conflicted: false,
            timed_out: false,
            oracle_rejected: false,
        };
        let v = outcome_to_json(&o);
        // The whole seal surfaces: diff/log hashes + the seal itself.
        assert_eq!(v["verify"]["kind"], "passed");
        assert_eq!(v["verify"]["diff_sha256"], seal.diff_sha256);
        assert_eq!(v["verify"]["log_sha256"], seal.log_sha256);
        assert_eq!(v["verify"]["seal"], seal.seal);
        assert_eq!(v["verify"]["verify_cmd"], "./scripts/check.sh");
        assert_eq!(v["integrated"], true);
    }

    #[test]
    fn cap_chars_truncates_and_marks() {
        let long = "x".repeat(50);
        assert_eq!(cap_chars(&long, 10).len(), 10 + "\n…[truncated]".len());
        assert!(cap_chars(&long, 10).ends_with("[truncated]"));
        assert_eq!(cap_chars("short", 100), "short");
    }

    #[tokio::test]
    async fn job_record_round_trips_through_serde() {
        let rec = JobRecord {
            id: "testjob".into(),
            status: JobStatus::Done,
            progress: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 2,
            request: JobRequest {
                task: "t".into(),
                cwd: "/tmp/x".into(),
                model: None,
                verify: None,
                max_parallel: None,
                yolo: true,
                deadline_minutes: None,
            },
            result: Some(json!({"ok": true})),
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: JobRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "testjob");
        assert_eq!(back.status, JobStatus::Done);
        assert!(back.request.yolo);
        assert_eq!(back.result, Some(json!({"ok": true})));
    }

    #[test]
    fn job_status_serde_uses_lowercase() {
        assert_eq!(
            serde_json::to_string(&JobStatus::Queued).unwrap(),
            "\"queued\""
        );
        assert_eq!(
            serde_json::to_string(&JobStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(serde_json::to_string(&JobStatus::Done).unwrap(), "\"done\"");
        assert_eq!(
            serde_json::to_string(&JobStatus::Error).unwrap(),
            "\"error\""
        );
    }
}
