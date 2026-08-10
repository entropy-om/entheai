//! The registered tool surface of the `entheai-mcp` server. Every tool is
//! `entheai_`-prefixed (opencode already has an `oracle` agent, so the bridge
//! tools must never collide). The table is the single source of truth for the
//! `tools/list` schema and the `tools/call` dispatch.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use serde_json::{json, Value};

use crate::board_tools;
use crate::dispatch_tool;
use crate::fanout_tool;
use crate::fleet_tool;
use crate::memory_tools;
use crate::run_tool;
use crate::skills_tools;

/// A tool's executor: `(arguments, server_cwd) → result`. Each tool resolves
/// its own `cwd` (default: the server's cwd) and loads the per-repo config.
pub type Handler =
    fn(Value, PathBuf) -> Pin<Box<dyn Future<Output = anyhow::Result<Value>> + Send>>;

/// Wrap an `async fn tool(args, server_cwd)` into a [`Handler`].
macro_rules! handler {
    ($f:path) => {
        |args: Value, cwd: PathBuf| Box::pin($f(args, cwd))
    };
}

/// One registered MCP tool.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub handler: Handler,
}

/// The `cwd` property shared by every tool that roots entheai somewhere.
fn cwd_prop() -> Value {
    json!({
        "type": "string",
        "description": "Repo/dir root entheai should work in (default: the server's cwd). entheai reads entheai.toml and installs skills here."
    })
}

/// Build an `inputSchema` (JSON Schema) from a properties map + required list.
fn schema(props: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
    })
}

/// All tools, in registration order (this is what `tools/list` returns).
pub fn all() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "entheai_run",
            description: "One-shot entheai model call: run {prompt} through the resolved model and return {answer, model, duration_ms}. Use for a second opinion from a different model, a classification/extraction micro-task, or (model: \"quantal/quantal\") offline native ternary inference. Non-interactive — never prompts for permissions.",
            input_schema: schema(
                json!({
                    "prompt": {"type": "string", "description": "The prompt / instruction to run."},
                    "cwd": cwd_prop(),
                    "model": {"type": "string", "description": "Model id \"<provider>/<model>\" (default: the repo config's default_model). \"quantal/quantal\" = offline ternary."},
                    "yolo": {"type": "boolean", "description": "Auto-approve everything (default false — the run uses an unattended auto policy anyway)."},
                    "timeout_secs": {"type": "number", "description": "Bail out after this many seconds (default 300)."},
                }),
                &["prompt"],
            ),
            handler: handler!(run_tool::entheai_run),
        },
        ToolDef {
            name: "entheai_fanout",
            description: "Decompose {task} and run parallel coder sub-agents in isolated git worktrees — the entheai fan-out engine, called AS A LIBRARY so every verified coder returns its full MergeSeal (diff_sha256/log_sha256/seal). Returns {job_id} immediately; poll entheai_job_status for {status, progress, result}. Long-running by nature — never block on the call itself. Non-interactive (yolo/auto policy; children can't answer stdin prompts).",
            input_schema: schema(
                json!({
                    "task": {"type": "string", "description": "The work to fan out (decomposed into parallel sub-tasks)."},
                    "cwd": cwd_prop(),
                    "model": {"type": "string", "description": "Orchestrator model id (default: the repo config's orchestrator/router model)."},
                    "verify": {"type": "string", "description": "Verify command per coder worktree (default: [fanout].verify or auto-detected ./scripts/check.sh)."},
                    "max_parallel": {"type": "number", "description": "Cap on concurrently running coders (default: [router].max_parallel)."},
                    "yolo": {"type": "boolean", "description": "Run coders under a yolo policy (default false — the fan-out uses an unattended auto ceiling policy)."},
                    "deadline_minutes": {"type": "number", "description": "Abort the whole job after this many minutes (default: no whole-job deadline; per-coder timeout from config)."},
                }),
                &["task", "cwd"],
            ),
            handler: handler!(fanout_tool::entheai_fanout),
        },
        ToolDef {
            name: "entheai_dispatch",
            description: "Dispatch ONE task to the entheai worker fleet over NATS (subscribe-before-dispatch, await WorkResult, apply the delta to fed/<session>-<index>) and return {status: committed|no-change|error|fell_through_local, branch, log, base, executed_on}. Honest: with no worker or on timeout it says executed_on: \"local\" — the caller must run the task locally, never pretend the fleet did it.",
            input_schema: schema(
                json!({
                    "task": {"type": "string", "description": "The task for the fleet worker (a coder runs it in the repo)."},
                    "cwd": cwd_prop(),
                    "role": {"type": "string", "description": "Worker sub-agent role (default \"coder\")."},
                    "deadline_secs": {"type": "number", "description": "How long to wait for a worker result before falling through (default: [federation].deadline_secs)."},
                }),
                &["task"],
            ),
            handler: handler!(dispatch_tool::entheai_dispatch),
        },
        ToolDef {
            name: "entheai_job_status",
            description: "Poll a background job started by entheai_fanout. Returns {status: queued|running|done|error, progress, result, request}. Jobs persist at ~/.cache/entheai-bridge/jobs/<id>.json.",
            input_schema: schema(
                json!({
                    "job_id": {"type": "string", "description": "The job id returned by entheai_fanout."},
                }),
                &["job_id"],
            ),
            handler: handler!(fanout_tool::entheai_job_status),
        },
        ToolDef {
            name: "entheai_fleet_status",
            description: "Live worker roster from the NATS presence heartbeat subscription: {workers: [{node_id, hostname, state, started_at_unix}], count, available}. Empty when federation is off or no worker is serving.",
            input_schema: schema(
                json!({
                    "cwd": cwd_prop(),
                }),
                &[],
            ),
            handler: handler!(fleet_tool::entheai_fleet_status),
        },
        ToolDef {
            name: "entheai_memory_search",
            description: "Read-only semantic/keyword search of the entheai memory store (namespaces: codebase|learnings|trajectories|tools|subagents). Ask the engine's brain before spending tokens.",
            input_schema: schema(
                json!({
                    "namespace": {"type": "string", "enum": ["codebase", "learnings", "trajectories", "tools", "subagents"]},
                    "query": {"type": "string", "description": "Free-text query."},
                    "limit": {"type": "number", "description": "Max results (default 10, capped 50)."},
                    "cwd": cwd_prop(),
                }),
                &["namespace", "query"],
            ),
            handler: handler!(memory_tools::entheai_memory_search),
        },
        ToolDef {
            name: "entheai_memory_stats",
            description: "Per-namespace entry counts of the entheai memory store (codebase|learnings|trajectories|tools|subagents) plus the total.",
            input_schema: schema(
                json!({
                    "cwd": cwd_prop(),
                }),
                &[],
            ),
            handler: handler!(memory_tools::entheai_memory_stats),
        },
        ToolDef {
            name: "entheai_memory_list",
            description: "List recent entries in one entheai memory namespace, newest first.",
            input_schema: schema(
                json!({
                    "namespace": {"type": "string", "enum": ["codebase", "learnings", "trajectories", "tools", "subagents"]},
                    "limit": {"type": "number", "description": "Max entries (default 10, capped 50)."},
                    "cwd": cwd_prop(),
                }),
                &["namespace"],
            ),
            handler: handler!(memory_tools::entheai_memory_list),
        },
        ToolDef {
            name: "entheai_skills_add",
            description: "Install a skill from a URL into the repo's skills/ dir (repo-scoped — resolves relative to cwd). Discovers <origin>/.well-known/skills.json, then /llms.txt, then the page.",
            input_schema: schema(
                json!({
                    "url": {"type": "string", "description": "The skill source URL (http/https)."},
                    "cwd": cwd_prop(),
                }),
                &["url"],
            ),
            handler: handler!(skills_tools::entheai_skills_add),
        },
        ToolDef {
            name: "entheai_skills_list",
            description: "List the skills discovered in the repo's skills dirs (relative to cwd): name, description, path.",
            input_schema: schema(
                json!({
                    "cwd": cwd_prop(),
                }),
                &[],
            ),
            handler: handler!(skills_tools::entheai_skills_list),
        },
        ToolDef {
            name: "entheai_board_list",
            description: "Read the tantric board (mlxquantlovefrom.com, GitHub-issues-backed in peterlodri-sec/mlxquantlovefrom.com): all lanes (backlog/burning/tantra/done) and their cards. Card = GitHub issue with a lane label. Public read; a TANTRIC_TOKEN_* env var is used when present.",
            input_schema: schema(
                json!({
                    "cwd": cwd_prop(),
                }),
                &[],
            ),
            handler: handler!(board_tools::entheai_board_list),
        },
        ToolDef {
            name: "entheai_board_add",
            description: "Create a card on the tantric board (mlxquantlovefrom.com): a GitHub issue with the lane label. Write access is restricted to the three board collaborators (peterlodri-sec, 8bit-wraith, standardgalactic), each with their own TANTRIC_TOKEN_PETER / TANTRIC_TOKEN_8BIT / TANTRIC_TOKEN_SG env var — errors clearly if none is set.",
            input_schema: schema(
                json!({
                    "title": {"type": "string", "description": "The card title."},
                    "lane": {"type": "string", "enum": ["backlog", "burning", "tantra", "done"], "description": "Lane to put the card in (default tantra)."},
                    "cwd": cwd_prop(),
                }),
                &["title"],
            ),
            handler: handler!(board_tools::entheai_board_add),
        },
    ]
}
