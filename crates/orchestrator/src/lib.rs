//! Fan-out orchestration: an orchestrator model decomposes a task into
//! independent sub-tasks, sub-agents (model-matched per role) run them in
//! parallel, and the orchestrator synthesizes a final answer.
//!
//! v1 (`run_fanout_readonly`): sub-agents share the process cwd and get a
//! READ-ONLY tool set (`read_file` + `search`) — safe for parallel
//! exploration/analysis. Used as-is when `root` isn't a git repo.
//!
//! v2 (`run_fanout`, the public entrypoint): each decomposed sub-task gets its
//! own coder sub-agent with a FULL (read/write/shell) tool set, running in an
//! ISOLATED `git worktree` (see [`worktree`]) so parallel writers never step on
//! each other. Each coder's worktree is committed, verified against the
//! resolved gate (`[fanout].verify`, else `./scripts/check.sh` — mandatory
//! unless `[fanout].verify_required = false`), and — if it committed and
//! verified clean — integrated onto a fresh integration branch carrying a
//! deterministic SHA-256 [`MergeSeal`] over its diff + verify log. Returns a
//! structured report instead of an extra synthesis LLM call.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use entheai_config::Config;
use entheai_core::EntheaiAgent;
use entheai_memory::{MemoryRuntime, MemoryScope};
use futures::stream::{self, StreamExt};
use serde::Deserialize;

pub mod agy;
pub mod copilot;
pub mod oracle;
pub mod pool;
pub mod repair_stop;
pub mod worktree;

pub use agy::AgyExecutor;
pub use copilot::CopilotExecutor;

pub use oracle::{
    oracle_for_config, Attestation, AttestationKind, FusionOracle, GateMode, NoOpOracle, Oracle,
    OracleAdjudication, OracleBackend, OracleContext, Phase, Verdict,
};

pub use pool::{WorkerId, WorkerPool, WorkerStatus, WorkerSummary};
pub use repair_stop::{Attempt, RepairDecision, RepairLedger, StopReason};

/// Lifecycle progress events emitted by [`run_fanout`] as it decomposes,
/// dispatches coders, and integrates their work. Consumers (e.g. the TUI) pass
/// an `UnboundedSender` to receive a live feed; `None` is a no-op producer
/// (used by the one-shot CLI path, which only cares about the final report).
#[derive(Debug, Clone)]
pub enum FanoutEvent {
    /// `root` isn't a git repo — falling back to the read-only v1 path.
    Fallback,
    /// The orchestrator decomposed the task into these (role, task) sub-tasks.
    Decomposed { tasks: Vec<(String, String)> },
    /// A coder sub-agent started work on its sub-task.
    CoderStarted {
        index: usize,
        role: String,
        task: String,
    },
    /// A coder sub-agent finished; `status` is a short human summary (e.g.
    /// "no changes", "verify failed", "verified", "changes committed (unverified)").
    CoderFinished {
        index: usize,
        committed: bool,
        status: String,
    },
    /// Integrating `branches` eligible coder branches onto a fresh branch.
    Integrating { branches: usize },
    /// Fan-out finished.
    Done {
        integration_branch: Option<String>,
        merged: usize,
        conflicted: usize,
    },
}

/// A strategy for running one coder sub-task. `run_fanout` uses this to
/// optionally offload coders to a remote worker fleet (F2.2); `None` = always
/// local (today's behavior). Kept NATS-agnostic — the impl lives in
/// `entheai-federation`, which depends on this crate for the trait.
#[async_trait::async_trait]
pub trait CoderExecutor: Send + Sync {
    /// Cheap check: is at least one worker available right now? When false,
    /// `run_fanout` skips remote dispatch entirely and runs every coder locally.
    async fn workers_available(&self) -> bool;

    /// Run the coder for `(role, task)` remotely, applying its changes into
    /// `worktree_path` as UNCOMMITTED working-tree changes (ready for the normal
    /// commit/verify/integrate path). `base_sha` is the worktree's base commit.
    /// Returns the coder's log on success, or `None` to fall back to local.
    async fn execute(
        &self,
        session: &str,
        index: usize,
        base_sha: &str,
        worktree_path: &Path,
        role: &str,
        task: &str,
    ) -> Option<String>;
}

/// One decomposed unit of work: a role (routes to a model) + its focused task.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SubTask {
    pub role: String,
    pub task: String,
}

/// A finished sub-agent's output (or its error text — a failed sub-agent never
/// aborts the batch; its failure is reported to the synthesizer as its output).
#[derive(Debug, Clone)]
pub struct SubResult {
    pub role: String,
    pub task: String,
    pub output: String,
}

/// Prompter used by fan-out sub-agents. Sub-agents run under a yolo policy, so
/// this is never actually consulted; it exists to satisfy `EntheaiAgent`'s
/// constructors, which require a prompter unconditionally.
struct AutoAllow;
#[async_trait::async_trait]
impl entheai_permission::Prompter for AutoAllow {
    async fn confirm(&mut self, _tool: &str, _args: &str) -> entheai_permission::Grant {
        entheai_permission::Grant::Allow
    }
}

/// A Policy for an unattended subagent: auto-approve tools at or below the parent's
/// tier ceiling, deny above (never Ask — subagents can't prompt).
pub fn ceiling_policy(parent: entheai_permission::Mode) -> entheai_permission::Policy {
    entheai_permission::Policy::with_ceiling(parent.ceiling())
}

/// Build the fan-out sub-agent/coder permission policy from config and environment.
fn fanout_policy(config: &Config) -> entheai_permission::Policy {
    let mode = if !config.fanout.mode.is_empty() {
        entheai_permission::Mode::parse(&config.fanout.mode)
    } else if let Ok(s) = std::env::var("ENTHEAI_MODE") {
        entheai_permission::Mode::parse(&s)
    } else if config.permission.yolo {
        entheai_permission::Mode::Yolo
    } else if !config.permission.fanout_auto_approve {
        entheai_permission::Mode::Plan
    } else {
        entheai_permission::Mode::Auto
    };
    ceiling_policy(mode)
}

/// Read-only tool set for sub-agents (no writes/shell → safe to run in parallel
/// against a shared cwd).
fn read_only_registry(root: &Path) -> entheai_tools::ToolRegistry {
    let mut r = entheai_tools::ToolRegistry::new();
    r.register(Box::new(entheai_tools::fs::ReadFile::new(
        root.to_path_buf(),
    )));
    r.register(Box::new(entheai_tools::search::Search::new(
        root.to_path_buf(),
    )));
    r
}

/// System prompt that asks the orchestrator to decompose a task into a JSON array.
const DECOMPOSE_SYSTEM: &str = "You are the orchestrator of a fan-out coding agent. \
Break the user's task into a small set of INDEPENDENT sub-tasks that can run in parallel. \
Each sub-task has a `role` (one of: explore, coder, reviewer, test, docs) and a concise `task` string. \
Sub-agents can only READ and SEARCH the codebase, so scope sub-tasks to analysis/exploration, not edits. \
Respond with ONLY a JSON array, e.g. [{\"role\":\"explore\",\"task\":\"map the auth module\"}]. No prose.";

fn decompose_messages(task: &str) -> (String, String) {
    (DECOMPOSE_SYSTEM.to_string(), task.to_string())
}

/// Decompose prompt for the v2 coder path (isolated git worktrees, full tools).
/// The read-only [`DECOMPOSE_SYSTEM`] tells the model to "scope sub-tasks to
/// analysis/exploration, not edits" — exactly wrong here, where sub-agents MAKE
/// the changes. Without a coder-oriented prompt a weak orchestrator model returns
/// an explore-only plan and the fan-out integrates nothing.
const DECOMPOSE_SYSTEM_CODER: &str = "You are the orchestrator of a fan-out coding agent. \
Break the user's task into a small set of INDEPENDENT sub-tasks that can run in parallel. \
Each sub-task has a `role` (one of: coder, explore, reviewer, test, docs) and a concise `task` string. \
Every sub-agent runs in its OWN isolated git worktree with full read/write/shell tools. \
Any change to the codebase MUST be performed by a `coder` sub-task that actually makes the edit; \
`explore`/`reviewer`/`test`/`docs` support it but never modify files. \
If the task requires changing code, include at least one `coder` sub-task that implements it. \
Respond with ONLY a JSON array, e.g. [{\"role\":\"coder\",\"task\":\"add a module doc comment to crates/foo/src/lib.rs\"}]. No prose.";

fn decompose_messages_coder(task: &str) -> (String, String) {
    (DECOMPOSE_SYSTEM_CODER.to_string(), task.to_string())
}

/// v2 safety net: the coder path exists to CHANGE code, so a run must contain at
/// least one `coder` sub-task. A weak orchestrator model sometimes returns an
/// explore-only (or empty) decomposition; without this guard the fan-out would
/// analyze the task and integrate nothing. Appends a single coder for the whole
/// task when the model produced none.
fn ensure_coder(mut subtasks: Vec<SubTask>, task: &str) -> Vec<SubTask> {
    if !subtasks
        .iter()
        .any(|s| s.role.eq_ignore_ascii_case("coder"))
    {
        subtasks.push(SubTask {
            role: "coder".to_string(),
            task: task.to_string(),
        });
    }
    subtasks
}

fn subagent_messages(role: &str, task: &str) -> (String, String) {
    let sys = format!(
        "You are a `{role}` sub-agent in a fan-out coding agent. Use read_file/search to \
         investigate, then report your findings concisely. You cannot modify files."
    );
    (sys, task.to_string())
}

/// Build the synthesis user message from the original task + all sub-results.
pub fn synthesis_user_message(task: &str, results: &[SubResult]) -> String {
    let mut s = format!("Original task:\n{task}\n\nSub-agent results:\n");
    for (i, r) in results.iter().enumerate() {
        s.push_str(&format!(
            "\n## Sub-agent {} [{}] — {}\n{}\n",
            i + 1,
            r.role,
            r.task,
            r.output
        ));
    }
    s.push_str("\nSynthesize a single, complete final answer for the user from these results.");
    s
}

fn synthesis_messages(task: &str, results: &[SubResult]) -> (String, String) {
    (
        "You are the orchestrator. Synthesize the sub-agent results into one clear final answer."
            .to_string(),
        synthesis_user_message(task, results),
    )
}

/// Extract a JSON array substring from possibly-prose/fenced model output.
fn extract_json_array(s: &str) -> Option<&str> {
    let start = s.find('[')?;
    let end = s.rfind(']')?;
    if end > start {
        Some(&s[start..=end])
    } else {
        None
    }
}

/// Parse the orchestrator's decomposition, skipping empties and capping at `max`.
pub fn parse_decomposition(content: &str, max: usize) -> Vec<SubTask> {
    let trimmed = content.trim();
    let slice = extract_json_array(trimmed).unwrap_or(trimmed);
    let parsed: Vec<SubTask> = serde_json::from_str(slice).unwrap_or_default();
    parsed
        .into_iter()
        .filter(|s| !s.role.trim().is_empty() && !s.task.trim().is_empty())
        .take(max)
        .collect()
}

/// An `Arc<Mutex<dyn Prompter>>` around `AutoAllow`, for the unattended,
/// no-tool-confirmation agents `orchestrate_once`/`run_subagent`/`run_coder_once` build.
fn auto_allow_prompter() -> Arc<tokio::sync::Mutex<dyn entheai_permission::Prompter>> {
    Arc::new(tokio::sync::Mutex::new(AutoAllow))
}

/// Run the orchestrator model once (empty registry = a single completion).
/// `system`, if `None`, falls back to the orchestrator identity prompt —
/// mirrors the old "prepend the identity prompt unless the caller already
/// set one" behavior, modeled as `Option` instead of message-list inspection.
async fn orchestrate_once(
    config: &Config,
    model_id: &str,
    system: Option<&str>,
    user: &str,
) -> anyhow::Result<String> {
    let default_system = entheai_router::orchestrator_system_prompt(config);
    let instruction = system.unwrap_or(&default_system);
    let agent = entheai_router::build_agent(
        model_id,
        config,
        Some(instruction),
        &entheai_tools::ToolRegistry::new(),
        Arc::new(fanout_policy(config)),
        auto_allow_prompter(),
    )?;
    agent.run_to_text(user).await
}

/// A per-leaf `MemoryScope` for a fan-out coder/sub-agent: keeps `base`'s
/// `session_id` (ties every leaf back to the caller's conversation session)
/// but overrides `task_id` (uniqued by `prefix`+`index`, so concurrently
/// running leaves never collide in retrieval/trajectory recording), `cwd`
/// (the leaf's own worktree/root, not the caller's), and `role`.
fn leaf_scope(
    base: &MemoryScope,
    prefix: &str,
    index: usize,
    cwd: &Path,
    role: &str,
) -> MemoryScope {
    MemoryScope {
        task_id: format!("{prefix}-{index}"),
        cwd: cwd.to_path_buf(),
        role: Some(role.to_string()),
        ..base.clone()
    }
}

/// Run one sub-agent to completion. Never returns Err — a failure is captured as
/// the sub-result's `output` so one bad sub-agent doesn't sink the whole batch.
/// When `memory` is `Some`, the agent is built directly via
/// `EntheaiAgent::new_with_memory` (bypassing the router, which has no
/// memory-aware variant) under a per-leaf [`leaf_scope`].
async fn run_subagent(
    config: &Config,
    root: &Path,
    st: SubTask,
    index: usize,
    memory: Option<Arc<MemoryRuntime>>,
    scope: &MemoryScope,
) -> SubResult {
    let output = async {
        let model_id = entheai_router::available_or_free(
            config,
            entheai_router::model_for_role(config, &st.role)?,
        );
        let (system, user) = subagent_messages(&st.role, &st.task);
        let agent = match memory {
            Some(mem) => EntheaiAgent::new_with_memory(
                &model_id,
                Some(&system),
                &config.inference,
                &config.providers,
                &read_only_registry(root),
                Arc::new(fanout_policy(config)),
                auto_allow_prompter(),
                config.router.max_turns as u32,
                mem,
                None,
                leaf_scope(scope, "fanout-readonly", index, root, &st.role),
                None,
            )?,
            None => entheai_router::build_agent(
                &model_id,
                config,
                Some(&system),
                &read_only_registry(root),
                Arc::new(fanout_policy(config)),
                auto_allow_prompter(),
            )?,
        };
        agent.run_to_text(&user).await
    }
    .await
    .unwrap_or_else(|e| format!("error: sub-agent failed: {e}"));
    SubResult {
        role: st.role,
        task: st.task,
        output,
    }
}

/// Read-only fan-out (v1): decompose → parallel read-only sub-agents (≤ router.max_parallel)
/// → synthesize. Falls back to a single orchestrator run if decomposition yields no sub-tasks.
/// Used directly when `root` isn't a git repo (v2's isolated worktrees need git).
/// `memory`/`scope` are forwarded to each sub-agent (see [`run_subagent`]); the
/// decompose/synthesis meta-calls on `orch_model` stay memory-free.
async fn run_fanout_readonly(
    config: &Config,
    root: &Path,
    task: &str,
    memory: Option<Arc<MemoryRuntime>>,
    scope: &MemoryScope,
) -> anyhow::Result<String> {
    let orch_model =
        entheai_router::available_or_free(config, entheai_router::orchestrator_model(config)?);

    // 1. Map + decompose.
    let mapped = entheai_mapper::Mapper::map(root, task, &[]).await;
    let (decompose_system, decompose_user) = decompose_messages(&mapped.render());
    let raw = orchestrate_once(
        config,
        &orch_model,
        Some(&decompose_system),
        &decompose_user,
    )
    .await?;
    let max_par = config.router.max_parallel.max(1);
    let subtasks = parse_decomposition(&raw, max_par);

    // Fallback: couldn't decompose → just run the task once on the orchestrator.
    // Uses `mapped.render()`, not raw `task`: `orchestrate_once` never registers any
    // tools, so a raw `@{file}` marker here would be a dead end the model can't resolve.
    if subtasks.is_empty() {
        return orchestrate_once(config, &orch_model, None, &mapped.render()).await;
    }

    // 2. Fan out, bounded by max_parallel.
    let results: Vec<SubResult> = stream::iter(subtasks.into_iter().enumerate())
        .map(|(i, st)| run_subagent(config, root, st, i, memory.clone(), scope))
        .buffer_unordered(max_par)
        .collect()
        .await;

    // 3. Synthesize. Same reasoning as the fallback above: the synthesis call has no
    // tool access either, so it needs the resolved file content, not a raw marker.
    let (synth_system, synth_user) = synthesis_messages(&mapped.render(), &results);
    orchestrate_once(config, &orch_model, Some(&synth_system), &synth_user).await
}

/// Full (read/write/edit/shell/search/todo) tool set for a coder sub-agent,
/// rooted at its own isolated worktree — safe to run in parallel because each
/// coder's `root` is a distinct `git worktree` checkout, not the shared cwd.
///
/// Uses the shared [`entheai_tools::register_builtins`] so the coders and the
/// CLI agent stay in lockstep — this is where the coders regained the `todo`
/// tool. Default limits preserve the exact shell/search behaviour they had.
fn write_registry(root: &Path) -> entheai_tools::ToolRegistry {
    let mut r = entheai_tools::ToolRegistry::new();
    entheai_tools::register_builtins(&mut r, root, entheai_tools::BuiltinLimits::default());
    r
}

fn coder_messages(role: &str, task: &str) -> (String, String) {
    let sys = format!(
        "You are a `{role}` sub-agent working in an ISOLATED git worktree. Make the necessary \
         code changes with write_file/run_shell to accomplish your task. Keep changes minimal \
         and focused."
    );
    (sys, task.to_string())
}

/// One coder sub-agent's finished run: its isolated worktree + what it produced.
/// Not yet committed/verified/integrated — see [`CoderOutcome`] for that.
struct CoderRun {
    index: usize,
    role: String,
    task: String,
    branch: String,
    path: PathBuf,
    output: String,
}

/// Run one coder sub-agent to completion against `worktree_path`: resolve its
/// model via the router, build the write-capable tool registry rooted at the
/// worktree, and run it under a yolo policy. Never returns `Err` — a failure
/// is captured as `"error: coder failed: {e}"` text so one bad coder never
/// aborts its caller. Standalone entry point for `entheai-worker`; also used
/// by [`run_coder`] (the in-process, `WorkerPool`-tracked dispatch path).
pub async fn run_coder_once(
    config: &Config,
    role: &str,
    task: &str,
    worktree_path: &Path,
) -> String {
    async {
        let model_id = entheai_router::available_or_free(
            config,
            entheai_router::model_for_role(config, role)?,
        );
        let (system, user) = coder_messages(role, task);
        let agent = entheai_router::build_agent(
            &model_id,
            config,
            Some(&system),
            &write_registry(worktree_path),
            Arc::new(fanout_policy(config)),
            auto_allow_prompter(),
        )?;
        agent.run_to_text(&user).await
    }
    .await
    .unwrap_or_else(|e| format!("error: coder failed: {e}"))
}

/// Like [`run_coder_once`], but for the in-process fan-out path: when `memory`
/// is `Some`, builds the coder agent directly via `EntheaiAgent::new_with_memory`
/// (bypassing the router, which has no memory-aware variant) under the given
/// per-leaf scope. `run_coder_once` itself stays memory-free — it also backs
/// the standalone `entheai-worker` binary, a separate process with no access
/// to the orchestrator's in-process `MemoryRuntime`.
async fn run_coder_local(
    config: &Config,
    role: &str,
    task: &str,
    worktree_path: &Path,
    memory: Option<(Arc<MemoryRuntime>, MemoryScope)>,
) -> String {
    async {
        let model_id = entheai_router::available_or_free(
            config,
            entheai_router::model_for_role(config, role)?,
        );
        let (system, user) = coder_messages(role, task);
        let agent = match memory {
            Some((mem, scope)) => EntheaiAgent::new_with_memory(
                &model_id,
                Some(&system),
                &config.inference,
                &config.providers,
                &write_registry(worktree_path),
                Arc::new(fanout_policy(config)),
                auto_allow_prompter(),
                config.router.max_turns as u32,
                mem,
                None,
                scope,
                None,
            )?,
            None => entheai_router::build_agent(
                &model_id,
                config,
                Some(&system),
                &write_registry(worktree_path),
                Arc::new(fanout_policy(config)),
                auto_allow_prompter(),
            )?,
        };
        agent.run_to_text(&user).await
    }
    .await
    .unwrap_or_else(|e| format!("error: coder failed: {e}"))
}

/// Run one coder sub-agent to completion inside its own worktree. Never returns
/// Err — a failure is captured as the run's `output`, mirroring [`run_subagent`],
/// so one bad coder doesn't sink the whole fan-out. Emits no events (its caller
/// [`run_coder_maybe_remote`] owns the `CoderStarted` event). When `memory` is
/// `Some`, runs via [`run_coder_local`] under a [`leaf_scope`]; otherwise falls
/// back to the memory-free [`run_coder_once`] (identical to before this param
/// existed).
async fn run_coder_inner(
    config: Arc<Config>,
    wt: worktree::Worktree,
    st: SubTask,
    memory: Option<Arc<MemoryRuntime>>,
    scope: MemoryScope,
    session: String,
) -> CoderRun {
    let output = match &memory {
        Some(mem) => {
            let leaf = leaf_scope(
                &scope,
                &format!("fanout-{session}"),
                wt.index,
                &wt.path,
                &st.role,
            );
            run_coder_local(
                &config,
                &st.role,
                &st.task,
                &wt.path,
                Some((Arc::clone(mem), leaf)),
            )
            .await
        }
        None => run_coder_once(&config, &st.role, &st.task, &wt.path).await,
    };
    CoderRun {
        index: wt.index,
        role: st.role,
        task: st.task,
        branch: wt.branch,
        path: wt.path,
        output,
    }
}

/// Run one coder either remotely (via the injected [`CoderExecutor`], which
/// applies the delta into the worktree) or locally. On any remote miss — no
/// executor, no worker, no result, no change, or an error — falls back to a
/// local [`run_coder_inner`], so a coder is never silently dropped. Owns the
/// `CoderStarted` event for both paths. `memory`/`scope` are forwarded to the
/// local fallback only — a remote worker runs out-of-process and can't share
/// the caller's in-process `MemoryRuntime`.
#[allow(clippy::too_many_arguments)]
async fn run_coder_maybe_remote(
    config: Arc<Config>,
    wt: worktree::Worktree,
    st: SubTask,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    remote: Option<(Arc<dyn CoderExecutor>, String)>,
    session: String,
    memory: Option<Arc<MemoryRuntime>>,
    scope: MemoryScope,
) -> CoderRun {
    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::CoderStarted {
            index: wt.index,
            role: st.role.clone(),
            task: st.task.clone(),
        });
    }
    if let Some((ex, base_sha)) = remote {
        if let Some(log) = ex
            .execute(&session, wt.index, &base_sha, &wt.path, &st.role, &st.task)
            .await
        {
            // The worker applied its delta into wt.path; steps 4–5 (commit /
            // verify / integrate) proceed exactly as for a local coder.
            return CoderRun {
                index: wt.index,
                role: st.role,
                task: st.task,
                branch: wt.branch,
                path: wt.path,
                output: log,
            };
        }
        log::info!("federation: coder {} fell back to local", wt.index);
    }
    run_coder_inner(config, wt, st, memory, scope, session).await
}

/// Where fan-out reports empirical failure trajectories (roadmap Phase 3.1:
/// "knowledge grows in the soil. Even the brutal notes of failure.").
///
/// The consumer's contract — implemented outside this crate (the prompt-
/// processing memory's raw store adapts to it), so the orchestrator stays
/// agnostic of which memory tier soaks up the failures. Implementations must
/// be best-effort: never fail or block the fan-out.
#[async_trait::async_trait]
pub trait TrajectorySink: Send + Sync {
    /// Ingest one failure: structured metadata + the raw verify traceback.
    async fn ingest_failure(&self, meta: serde_json::Value, trace: &str);

    /// A coder's diff passed the empirical gate and was sealed (roadmap 3.2):
    /// the positive experience signal. Default: no-op — sinks that only soak
    /// up failures need not care.
    async fn ingest_sealed_success(&self, _meta: serde_json::Value) {}
}

/// Cryptographic seal binding a coder's committed diff to the empirical verify
/// log that earned it integration (human_todo.md Phase 2.1 / frozen/verification.md:
/// self-reported success is worthless without verified execution evidence).
///
/// Deterministic: same diff + same verify output ⇒ same seal, no timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeSeal {
    /// SHA-256 (hex) of `git diff <base>..HEAD` in the coder's worktree.
    pub diff_sha256: String,
    /// SHA-256 (hex) of the verify command's combined stderr+stdout.
    pub log_sha256: String,
    /// SHA-256 (hex) over `"<diff_sha256>:<log_sha256>"` — the seal itself.
    pub seal: String,
    /// The verify command that produced the log (empirical provenance).
    pub verify_cmd: String,
}

impl MergeSeal {
    /// Build a seal from raw diff bytes and raw verify-log bytes.
    pub fn compute(diff: &[u8], log: &[u8], verify_cmd: &str) -> Self {
        let diff_sha256 = sha256_hex(diff);
        let log_sha256 = sha256_hex(log);
        let seal = sha256_hex(format!("{diff_sha256}:{log_sha256}").as_bytes());
        Self {
            diff_sha256,
            log_sha256,
            seal,
            verify_cmd: verify_cmd.to_string(),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Outcome of running the resolved verify command in a coder's worktree.
#[derive(Debug, Clone)]
pub enum VerifyStatus {
    /// The coder made no commit — nothing to verify.
    NoChanges,
    /// No verify command resolved and `[fanout].verify_required = false` —
    /// integrated as-is (legacy lax mode).
    Skipped,
    /// No verify command resolved (neither `[fanout].verify` nor
    /// `./scripts/check.sh`) while verification is required — NOT integrated.
    Unverifiable,
    /// The verify command exited successfully; carries the deterministic
    /// SHA-256 seal over the committed diff + empirical verify log.
    Passed(MergeSeal),
    /// The verify command failed; carries the tail of its combined stderr+stdout.
    Failed(String),
}

/// Human status line for one coder's `FanoutEvent::CoderFinished`. Matches on
/// `verify` FIRST: a commit failure sets `committed = false` with
/// `VerifyStatus::Failed(..)` (see the commit_result match above) and must
/// report as that, not fall into a generic "no changes" just because
/// `committed` happens to be false too — that conflation is exactly what
/// silently hid a real commit failure behind the benign "coder made no edits"
/// case.
fn coder_status_label(committed: bool, verify: &VerifyStatus) -> &'static str {
    match verify {
        VerifyStatus::Failed(_) if !committed => "commit failed — not integrated",
        VerifyStatus::Failed(_) => "verify failed",
        _ if !committed => "no changes",
        VerifyStatus::Passed(_) => "verified + sealed",
        VerifyStatus::Skipped => "changes committed (unverified)",
        VerifyStatus::Unverifiable => "unverifiable — not integrated",
        VerifyStatus::NoChanges => "no changes",
    }
}

/// Last `n` chars of `s` (char-boundary safe — a byte-index slice could split a
/// multi-byte UTF-8 char, e.g. mid-emoji in a test's output).
fn tail_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    if total <= n {
        s.to_string()
    } else {
        s.chars().skip(total - n).collect()
    }
}

/// Run the resolved verify command (if any) in `path` to decide whether a
/// coder's changes are safe to integrate.
///
/// `cmd = None` means no command resolved (neither `[fanout].verify` nor
/// `./scripts/check.sh`): `required` then decides between [`VerifyStatus::Unverifiable`]
/// (mandatory gate — not integrated) and [`VerifyStatus::Skipped`] (legacy lax mode).
/// On success the committed diff against `base` and the raw verify log are
/// hashed into a deterministic [`MergeSeal`].
/// What `verify_worktree` runs: either a literal shell command — the
/// operator's own `[fanout].verify`, trusted as configured — or the pinned
/// content of the auto-detected `./scripts/check.sh` AS IT EXISTED AT `base`.
/// The auto-detect path is a coder-writable file inside the very worktree
/// being judged: without pinning, a coder could "pass" its own gate by
/// editing `scripts/check.sh` to `exit 0`, silently defeating the
/// "sub-agents cannot self-report success" guarantee `verify_required`
/// exists for. See [`resolve_verify_command`].
#[derive(Clone)]
enum VerifyCommand {
    Shell(String),
    PinnedScript(Vec<u8>),
}

impl VerifyCommand {
    fn describe(&self) -> String {
        match self {
            VerifyCommand::Shell(cmd) => cmd.clone(),
            VerifyCommand::PinnedScript(_) => "./scripts/check.sh (pinned to base)".to_string(),
        }
    }
}

/// Resolve what to run for `verify_worktree`, pinning the auto-detected
/// script against tampering (see [`VerifyCommand`]). An explicit
/// `[fanout].verify` is always trusted as the operator configured it — only
/// the parameterless auto-detect path is pinned. Falls back to trusting the
/// worktree's own copy when the script didn't exist at `base` (the coder's
/// task legitimately added it) or `git show` itself fails for any reason —
/// never harder-fails verification over this.
async fn resolve_verify_command(
    config: &entheai_config::Config,
    root: &Path,
    base: &str,
) -> Option<VerifyCommand> {
    let cmd = config.fanout.resolve_verify(root)?;
    if config.fanout.verify.is_some() {
        return Some(VerifyCommand::Shell(cmd));
    }
    // Auto-detected: `cmd` is exactly "./scripts/check.sh" per
    // `FanoutConfig::resolve_verify`. Pin it to the base commit's content.
    let pinned = tokio::process::Command::new("git")
        .args(["show", &format!("{base}:scripts/check.sh")])
        .current_dir(root)
        .kill_on_drop(true)
        .output()
        .await;
    match pinned {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => {
            Some(VerifyCommand::PinnedScript(out.stdout))
        }
        _ => Some(VerifyCommand::Shell(cmd)),
    }
}

async fn verify_worktree(
    path: &Path,
    base: &str,
    verify: Option<VerifyCommand>,
    required: bool,
    timeout: Duration,
) -> VerifyStatus {
    let Some(verify) = verify else {
        return if required {
            VerifyStatus::Unverifiable
        } else {
            VerifyStatus::Skipped
        };
    };
    let cmd = verify.describe();
    // Runs sequentially, one worktree at a time, OUTSIDE the pool's own
    // per-coder timeout tracking (that only bounds the coder's LLM run,
    // which already joined by the time this fires) — with no bound of its
    // own a hung `./scripts/check.sh`/`cargo test` blocked the whole fan-out
    // indefinitely, and `kill_on_drop` ensures a cancelled run doesn't orphan
    // the verify subprocess (and whatever it spawns).
    let spawn = async move {
        match verify {
            VerifyCommand::Shell(cmd) => {
                tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmd)
                    .current_dir(path)
                    .kill_on_drop(true)
                    .output()
                    .await
            }
            VerifyCommand::PinnedScript(script) => {
                use tokio::io::AsyncWriteExt;
                let mut child = tokio::process::Command::new("sh")
                    .arg("-s")
                    .current_dir(path)
                    .kill_on_drop(true)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()?;
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(&script).await?;
                    // Drop before `wait_with_output` so `sh -s` sees EOF and exits.
                }
                child.wait_with_output().await
            }
        }
    };
    match tokio::time::timeout(timeout, spawn).await {
        Err(_) => VerifyStatus::Failed(format!(
            "verify command `{cmd}` timed out after {timeout:?}"
        )),
        Ok(Ok(output)) if output.status.success() => {
            let mut log = output.stderr;
            log.extend_from_slice(&output.stdout);
            let diff = tokio::process::Command::new("git")
                .args(["diff", base, "HEAD"])
                .current_dir(path)
                .output()
                .await
                .map(|o| o.stdout)
                .unwrap_or_default();
            VerifyStatus::Passed(MergeSeal::compute(&diff, &log, &cmd))
        }
        Ok(Ok(output)) => {
            // Carry the FULL combined log: the trajectory sink (roadmap 3.1)
            // wants the raw traceback; display sites tail it themselves.
            let mut combined = String::from_utf8_lossy(&output.stderr).into_owned();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            VerifyStatus::Failed(combined)
        }
        Ok(Err(e)) => VerifyStatus::Failed(format!("failed to spawn verify command `{cmd}`: {e}")),
    }
}

/// A coder's fully-resolved outcome: what it produced, whether it committed,
/// how it verified, and where it ended up (integrated / conflicted / left alone).
pub struct CoderOutcome {
    pub index: usize,
    pub role: String,
    pub task: String,
    pub branch: String,
    /// The sub-agent's text output.
    pub output: String,
    pub committed: bool,
    pub verify: VerifyStatus,
    /// Final: merged into the integration branch.
    pub integrated: bool,
    /// Hit a merge conflict at integrate time (overrides `integrated`).
    pub conflicted: bool,
    /// The coder timed out or was killed mid-run — its partial work was
    /// deliberately NOT committed or integrated (Bug 2). Takes precedence over
    /// every other status when the report is rendered.
    pub timed_out: bool,
    /// The Oracle rejected this coder's diff under gate=block — its branch was
    /// excluded from integration (kept alive for recovery, like a conflict).
    pub oracle_rejected: bool,
}

/// A structured fan-out run: the human-readable report PLUS the per-coder
/// outcomes (which carry the full [`MergeSeal`] — `diff_sha256`/`log_sha256`/
/// `seal` — something the text report only fingerprints) and the run identity.
/// This is what makes library consumers (e.g. the `entheai-mcp` bridge) able to
/// return the full MergeSeal instead of shelling out + parsing text.
pub struct FanoutRun {
    /// The rendered markdown report ([`format_v2_report`] output).
    pub report: String,
    /// The run's session id (also the worktree/`fanout/<session>/<n>` prefix).
    pub session: String,
    /// The base commit every coder branched from (and the seal's diff base).
    pub base: String,
    /// One entry per decomposed sub-task, in order; `VerifyStatus::Passed`
    /// entries carry the deterministic SHA-256 seal over their diff + verify log.
    pub outcomes: Vec<CoderOutcome>,
}

/// Fan-out entrypoint (v2): decompose → one isolated `git worktree` + coder
/// sub-agent per sub-task, run in parallel (≤ `router.max_parallel`) → commit +
/// optionally verify each worktree → integrate the eligible branches onto a
/// fresh integration branch → return a structured report (no extra LLM call).
///
/// Falls back to the read-only v1 fan-out ([`run_fanout_readonly`]) when `root`
/// isn't a git repo (isolated worktrees require one).
///
/// `memory`/`scope`, when `memory` is `Some`, give every fan-out leaf
/// (sub-agent or coder) pre-task retrieval/frozen-node injection and
/// post-task trajectory recording under a per-leaf [`leaf_scope`] — the
/// `orchestrate_once` decompose/synthesis meta-calls stay memory-free.
///
/// The detailed variant ([`run_fanout_detailed`]) returns the same report plus
/// the structured outcomes (full MergeSeal); library consumers that need the
/// seal use that. This thin wrapper keeps the `anyhow::Result<String>` contract
/// the CLI/TUI callers rely on.
#[allow(clippy::too_many_arguments)]
pub async fn run_fanout(
    config: &Config,
    root: &Path,
    task: &str,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    pool: Arc<WorkerPool>,
    executor: Option<Arc<dyn CoderExecutor>>,
    oracle: Option<Arc<dyn Oracle>>,
    memory: Option<Arc<MemoryRuntime>>,
    scope: MemoryScope,
    trajectories: Option<Arc<dyn TrajectorySink>>,
) -> anyhow::Result<String> {
    Ok(run_fanout_detailed(
        config,
        root,
        task,
        events,
        pool,
        executor,
        oracle,
        memory,
        scope,
        trajectories,
    )
    .await?
    .report)
}

/// RAII tracking for spawned-but-not-yet-joined [`WorkerId`]s. `run_fanout_detailed`
/// is itself an aborted future when the caller cancels a run (TUI `Action::CancelRun`
/// aborts the task running it; the MCP fanout tool wraps it in a `tokio::time::timeout`),
/// but the coders it spawned via `pool.spawn` are detached `tokio::spawn` tasks owned
/// by the long-lived `WorkerPool` — dropping this future does NOT stop them. Without
/// this guard a cancelled run left every still-running coder issuing LLM calls and
/// writing into worktrees for up to `coder_timeout_secs`, while its own worktree
/// directories were being force-removed out from under it by `WorktreeGuard`'s drop.
///
/// `track` on spawn, `joined` right after a normal `pool.join` + `pool.reap`; whatever
/// remains in `ids` when this drops (including via an unwound/aborted future) gets
/// `pool.stop` + `pool.reap`.
struct PendingWorkers<'a> {
    pool: &'a WorkerPool,
    ids: Vec<WorkerId>,
}

impl<'a> PendingWorkers<'a> {
    fn new(pool: &'a WorkerPool) -> Self {
        Self {
            pool,
            ids: Vec::new(),
        }
    }

    fn track(&mut self, id: WorkerId) {
        self.ids.push(id);
    }

    fn joined(&mut self, id: WorkerId) {
        self.ids.retain(|&i| i != id);
    }
}

impl Drop for PendingWorkers<'_> {
    fn drop(&mut self) {
        for &id in &self.ids {
            self.pool.stop(id);
            self.pool.reap(id);
        }
    }
}

/// The structured fan-out entrypoint: identical execution to [`run_fanout`],
/// but returns the report together with the run identity and the per-coder
/// outcomes — so callers get the full [`MergeSeal`] per verified coder, not
/// just the 12-hex fingerprint the report renders.
#[allow(clippy::too_many_arguments)]
pub async fn run_fanout_detailed(
    config: &Config,
    root: &Path,
    task: &str,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    pool: Arc<WorkerPool>,
    executor: Option<Arc<dyn CoderExecutor>>,
    oracle: Option<Arc<dyn Oracle>>,
    memory: Option<Arc<MemoryRuntime>>,
    scope: MemoryScope,
    trajectories: Option<Arc<dyn TrajectorySink>>,
) -> anyhow::Result<FanoutRun> {
    if !worktree::is_git_repo(root).await {
        if let Some(tx) = &events {
            let _ = tx.send(FanoutEvent::Fallback);
        }
        let out = run_fanout_readonly(config, root, task, memory, &scope).await?;
        return Ok(FanoutRun {
            report: format!("(not a git repo — read-only fan-out)\n\n{out}"),
            session: String::new(),
            base: String::new(),
            outcomes: Vec::new(),
        });
    }

    let orch_model =
        entheai_router::available_or_free(config, entheai_router::orchestrator_model(config)?);
    let max_par = config.router.max_parallel.max(1);

    // 1. Map + decompose.
    let mapped = entheai_mapper::Mapper::map(root, task, &[]).await;
    let (decompose_system, decompose_user) = decompose_messages_coder(&mapped.render());
    let raw = orchestrate_once(
        config,
        &orch_model,
        Some(&decompose_system),
        &decompose_user,
    )
    .await?;
    // The v2 coder path exists to CHANGE code, so guarantee at least one `coder`
    // sub-task: a weak orchestrator model sometimes returns an explore-only (or
    // empty) plan, which would analyze the task and integrate nothing. `ensure_coder`
    // appends a coder for the whole task when none is present. `mapped.render()` (not
    // the raw task) carries the resolved file context a lone coder needs.
    let subtasks = ensure_coder(parse_decomposition(&raw, max_par), &mapped.render());
    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::Decomposed {
            tasks: subtasks
                .iter()
                .map(|s| (s.role.clone(), s.task.clone()))
                .collect(),
        });
    }

    // ── Oracle G1 (advisory): adjudicate the decompose plan. Never blocks by
    // default (gate=advisory); records the verdict so it lands in the report.
    if let Some(o) = &oracle {
        let ctx = OracleContext {
            task: task.to_string(),
            mapped_files: mapped
                .file_chunks
                .iter()
                .map(|c| c.path.to_string_lossy().into_owned())
                .collect(),
            ..OracleContext::default()
        };
        match o.adjudicate(Phase::Decompose, &ctx).await {
            Ok(adj) => {
                if let Verdict::Rework { reason, .. } = &adj.verdict {
                    log::info!("oracle G1 advisory: rework suggested for decompose plan: {reason}");
                }
            }
            Err(e) => log::warn!("oracle G1 failed (advisory, continuing): {e}"),
        }
    }

    let session = uuid::Uuid::new_v4().simple().to_string();
    let base = worktree::resolve_base(root, "HEAD").await?;

    // Offload coders to the fleet only when an executor is wired AND a worker is
    // actually available right now; otherwise every coder runs locally (the
    // unchanged path). This one presence check gates the whole batch.
    let remote: Option<(Arc<dyn CoderExecutor>, String)> = match &executor {
        Some(ex) if ex.workers_available().await => Some((ex.clone(), base.clone())),
        _ => None,
    };
    let wt_pool = worktree::WorktreePool::new(root, &session, &base).await?;

    // A scope guard owns every worktree created below and, on EVERY exit from
    // here on — normal return, a `?` early-return, or a panic — removes their
    // directories plus the pool's temp dir, so worktrees never leak into the
    // user's real repo across runs. It deletes only branches later marked merged
    // (`guard.mark_merged`); unmerged coder branches (conflicted / verify-failed /
    // timed-out / no-change) are kept alive for recovery.
    let mut guard = worktree::WorktreeGuard::new(wt_pool);

    // Tracks every coder WorkerId spawned below that hasn't been joined yet.
    // Declared right AFTER `guard` so it drops FIRST on unwind (including a
    // dropped/aborted future when the TUI's `Action::CancelRun` aborts this
    // whole `run_fanout` task) — Drop calls `pool.stop`/`pool.reap` on every
    // still-pending id, killing the coder subprocess/LLM call BEFORE `guard`'s
    // drop force-removes the worktree it was writing into. Without this, a
    // cancelled fan-out left detached coder tasks (owned by the long-lived
    // `pool`, not this future) running for up to `coder_timeout_secs` against
    // directories that no longer existed.
    let mut pending = PendingWorkers::new(&pool);

    // 2. Create one worktree per sub-task, sequentially (git worktree creation
    // isn't safe to parallelize against the same root repo).
    let mut wts: Vec<(worktree::Worktree, SubTask)> = Vec::with_capacity(subtasks.len());
    for (i, st) in subtasks.into_iter().enumerate() {
        let wt = guard.create(i).await?;
        wts.push((wt, st));
    }

    // 3. Dispatch coders through the WorkerPool (tracked, cancellable,
    // timeout-bounded) and collect their outcomes in the same order they were
    // spawned — no re-sort needed here, unlike the old buffer_unordered dispatch.
    let coder_timeout = Duration::from_secs(config.fanout.coder_timeout_secs);
    let config_arc = Arc::new(config.clone());
    let mut worker_ids: Vec<(WorkerId, worktree::Worktree, SubTask)> =
        Vec::with_capacity(wts.len());
    for (wt, st) in wts.iter().cloned() {
        let id = pool.spawn(
            st.role.clone(),
            st.task.clone(),
            coder_timeout,
            run_coder_maybe_remote(
                Arc::clone(&config_arc),
                wt.clone(),
                st.clone(),
                events.clone(),
                remote.clone(),
                session.clone(),
                memory.clone(),
                scope.clone(),
            ),
        );
        pending.track(id);
        worker_ids.push((id, wt, st));
    }

    // A coder that timed out (via `coder_timeout_secs`) or was killed (via
    // `/workers`) comes back as a `None` join. Its worktree holds whatever
    // half-written files the cancelled agent left, so it must NOT be committed or
    // integrated (Bug 2); record its index here so step 4 skips it.
    let mut runs: Vec<CoderRun> = Vec::with_capacity(worker_ids.len());
    let mut timed_out: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (id, wt, st) in worker_ids {
        let run = match pool.join(id).await {
            Some(run) => run,
            None => {
                let reason = match pool.status(id) {
                    Some(WorkerStatus::Killed) => "coder killed (stopped via /workers)",
                    _ => "coder timed out",
                };
                timed_out.insert(wt.index);
                CoderRun {
                    index: wt.index,
                    role: st.role,
                    task: st.task,
                    branch: wt.branch,
                    path: wt.path,
                    output: format!("error: {reason}"),
                }
            }
        };
        // Reap now that we've joined this worker AND read its final status —
        // otherwise the long-lived pool grows by `subtasks.len()` every run and
        // `/workers` keeps listing finished coders (Bug 4). Workers not yet joined
        // stay tracked, so `/workers` still shows in-flight coders during the run.
        pool.reap(id);
        pending.joined(id); // already joined + reaped — PendingWorkers::drop must skip it
        runs.push(run);
    }

    // 4. Commit + verify each worktree, sequentially (each is a separate git
    // invocation against a distinct worktree, but keeping this sequential keeps
    // output/ordering simple and avoids piling up concurrent `sh -c` verify runs).
    // Resolved once, outside the loop — `base`/`config` are loop-invariant, and
    // the pinned base-commit script content (see `VerifyCommand`) is the same
    // for every coder in this run.
    let verify_command = resolve_verify_command(config, root, &base).await;
    let mut outcomes: Vec<CoderOutcome> = Vec::with_capacity(runs.len());
    let mut eligible_branches: Vec<String> = Vec::new();
    for run in runs {
        // Bug 2: a timed-out/killed coder left partial, unverified work in its
        // worktree. Do NOT commit or integrate it — `commit_all` would snapshot
        // half-written files and, with `[fanout].verify` defaulting to Skipped,
        // they'd land on the integration branch marked "integrated ✓". Report it
        // as failed and move on; its branch is never eligible, so the guard keeps
        // it (unmerged).
        if timed_out.contains(&run.index) {
            if let Some(tx) = &events {
                let _ = tx.send(FanoutEvent::CoderFinished {
                    index: run.index,
                    committed: false,
                    status: "timed out — not integrated".to_string(),
                });
            }
            outcomes.push(CoderOutcome {
                index: run.index,
                role: run.role,
                task: run.task,
                branch: run.branch,
                output: run.output,
                committed: false,
                verify: VerifyStatus::NoChanges,
                integrated: false,
                conflicted: false,
                timed_out: true,
                oracle_rejected: false,
            });
            continue;
        }
        // A commit failure (repo pre-commit hook rejecting the edit, gpgsign
        // with no agent, ...) is NOT "no changes" — the coder's edits are real
        // and uncommitted, and this worktree is about to be force-removed by
        // `WorktreeGuard`. Distinguish it (`VerifyStatus::Failed`, excluded from
        // integration just like a failed verify) instead of collapsing it into
        // the benign "coder made no edits" case via `.unwrap_or(false)`.
        let commit_result = worktree::commit_all(
            &run.path,
            // `run.task` can be `mapped.render()` — the WHOLE resolved prompt incl.
            // `@{file}` contents — when the model returned no coder and
            // `ensure_coder` filled in a lone one; an uncapped commit message
            // then risks E2BIG on `git commit -m`, which (before this cap) would
            // fail the commit for a reason unrelated to the coder's actual diff.
            &format!(
                "entheai fan-out [{}]: {}",
                run.role,
                cap_str(&run.task, 2000)
            ),
        )
        .await;
        let (committed, verify) = match commit_result {
            Ok(true) => (
                true,
                verify_worktree(
                    &run.path,
                    &base,
                    verify_command.clone(),
                    config.fanout.verify_required,
                    coder_timeout,
                )
                .await,
            ),
            Ok(false) => (false, VerifyStatus::NoChanges),
            Err(e) => (false, VerifyStatus::Failed(format!("commit failed: {e}"))),
        };
        // Roadmap 3.1/3.2: execution outcomes are soil, not noise. A failure
        // feeds its raw traceback to the trajectory sink; a sealed success
        // feeds the positive experience signal. Both best-effort.
        //
        // Repair seam (honest): a `Failed` leaf composts here and is NOT
        // retried — entheai verifies once. When a repair *actor* lands
        // (re-dispatch a leaf to fix a failed diff, then re-verify), govern the
        // loop with [`repair_stop::RepairLedger`]: keep the best-verified
        // incumbent, and stop once another repair has no evidenced marginal
        // value — a fallible verifier + blind repair can destroy a correct
        // state (VRR-Stop, Wu et al. 2026). The decision exists; the actor does
        // not yet.
        if let Some(sink) = &trajectories {
            match &verify {
                VerifyStatus::Failed(trace) => {
                    let meta = serde_json::json!({
                        "source": "fanout-verify",
                        "session": session,
                        "role": run.role,
                        "task": run.task,
                        "branch": run.branch,
                        "base": base,
                        "verify_cmd": verify_command.as_ref().map(VerifyCommand::describe),
                    });
                    sink.ingest_failure(meta, trace).await;
                }
                VerifyStatus::Passed(seal) => {
                    let meta = serde_json::json!({
                        "source": "fanout-verify",
                        "session": session,
                        "role": run.role,
                        "task": run.task,
                        "branch": run.branch,
                        "seal": seal.seal,
                    });
                    sink.ingest_sealed_success(meta).await;
                }
                _ => {}
            }
        }
        if let Some(tx) = &events {
            let _ = tx.send(FanoutEvent::CoderFinished {
                index: run.index,
                committed,
                status: coder_status_label(committed, &verify).to_string(),
            });
        }
        // ── Oracle G2 (advisory): adjudicate the coder's diff. ONLY gates
        // Passed/Skipped coders (NoChanges/Failed/Unverifiable already excluded
        // downstream). Diff-fallback — never stalls waiting for attestations.
        // Concurrent: runs inside the coder loop's own task. Under gate=block,
        // a high-confidence Reject/Rework excludes the branch from integration
        // (kept alive for recovery, like a conflict).
        let mut oracle_rejected = false;
        if let Some(o) = &oracle {
            // The coder's actual diff, not its self-reported prose (`run.output`)
            // — an adjudicator judging on the latter (or, before this fix, on
            // nothing but `diffs.len()` — see oracle.rs native_adjudicate) could
            // reject/approve a coder on a verdict grounded in nothing it changed.
            // Empty when nothing was committed (matches `branch_diff`'s own
            // "identical to base" contract) — never an error, so this always
            // has *some* diffs entry.
            let diff = worktree::branch_diff(root, &base, &run.branch)
                .await
                .unwrap_or_default();
            let ctx = OracleContext {
                task: run.task.clone(),
                mapped_files: Vec::new(),
                diffs: vec![(run.branch.clone(), diff)],
                // The eBPF sphere's kernel truth, if the sidecar is running
                // (coders=fleet). Empty on the local path → diff-fallback.
                attestations: oracle::sphere_attestations(),
                ..OracleContext::default()
            };
            match o.adjudicate(Phase::CoderDiff, &ctx).await {
                Ok(adj) => {
                    let mut reason = String::new();
                    if let Verdict::Reject { reason: r } = &adj.verdict {
                        reason = r.clone();
                        log::info!(
                            "oracle G2 advisory: coder {} ({}) flagged: {}",
                            run.index,
                            run.role,
                            r
                        );
                    }
                    if o.would_block(&adj) {
                        oracle_rejected = true;
                        log::warn!(
                            "oracle G2 BLOCK: coder {} ({}) excluded from integration ({reason})",
                            run.index,
                            run.role
                        );
                    }
                }
                Err(e) => log::warn!("oracle G2 failed (advisory, continuing): {e}"),
            }
        }
        let integrated = !oracle_rejected
            && committed
            && matches!(verify, VerifyStatus::Skipped | VerifyStatus::Passed(_));
        if integrated {
            eligible_branches.push(run.branch.clone());
        }
        outcomes.push(CoderOutcome {
            index: run.index,
            role: run.role,
            task: run.task,
            branch: run.branch,
            output: run.output,
            committed,
            verify,
            integrated,
            conflicted: false,
            oracle_rejected,
            timed_out: false,
        });
    }

    // 5. Integrate the eligible branches onto a fresh integration branch. A
    // branch can still conflict here even though it verified clean in
    // isolation (two coders touching the same lines) — reflect that in the
    // per-branch status, overriding `integrated` for anything that conflicted.
    let integration = if eligible_branches.is_empty() {
        None
    } else {
        // ── Oracle G3 (advisory): adjudicate the integration candidate set.
        // Conditional by default — when no Oracle is wired this is a no-op;
        // advisory mode logs the verdict, never blocks integration.
        if let Some(o) = &oracle {
            let ctx = OracleContext {
                task: task.to_string(),
                mapped_files: Vec::new(),
                diffs: eligible_branches
                    .iter()
                    .map(|b| (b.clone(), String::new()))
                    .collect(),
                ..OracleContext::default()
            };
            match o.adjudicate(Phase::Integration, &ctx).await {
                Ok(adj) => {
                    if let Verdict::Reject { reason } = &adj.verdict {
                        log::info!("oracle G3 advisory: integration flagged: {reason}");
                    }
                }
                Err(e) => log::warn!("oracle G3 failed (advisory, continuing): {e}"),
            }
        }
        if let Some(tx) = &events {
            let _ = tx.send(FanoutEvent::Integrating {
                branches: eligible_branches.len(),
            });
        }
        let integration = worktree::integrate(
            root,
            &base,
            &format!("entheai/{session}/integration"),
            &eligible_branches,
        )
        .await?;
        // Bug 1: only branches that actually merged are safe to delete on cleanup
        // (their commits survive via the integration merge). Mark them so the
        // guard `-D`s exactly those; everything else — including branches that
        // verified clean but then CONFLICTED here — is kept alive for recovery,
        // matching the "left on branch …" lines the report prints below.
        guard.mark_merged(integration.merged.iter().cloned());
        for outcome in outcomes.iter_mut() {
            if integration.conflicted.contains(&outcome.branch) {
                outcome.integrated = false;
                outcome.conflicted = true;
            }
        }
        Some(integration)
    };

    // 6. Worktree/branch cleanup is owned by `guard`: it runs when the guard
    // drops at the end of this function (and on any early-return/panic above),
    // removing every worktree DIRECTORY but deleting only merged branches (Bug 1),
    // and dropping the pool temp dir (Bug 5). The integration branch lives in the
    // root repo, not a worktree, so it is untouched and kept for review.

    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::Done {
            integration_branch: integration.as_ref().map(|i| i.branch.clone()),
            merged: integration.as_ref().map(|i| i.merged.len()).unwrap_or(0),
            conflicted: integration
                .as_ref()
                .map(|i| i.conflicted.len())
                .unwrap_or(0),
        });
    }

    let mut report = format_v2_report(task, &base, &session, &outcomes, integration.as_ref());

    // Roadmap 5.1: the recursive-development path (entheai developing entheai
    // via agy) leaves a transparent turn ledger and audits its own integrated
    // diff against AGENTS.md — the flywheel checks itself after every spin.
    if config.fanout.executor == "agy" {
        log_recursive_turns(root, &session, &outcomes);
        if let Some(i) = &integration {
            if !i.merged.is_empty() {
                let verdict = recursive_self_audit(config, root, &i.diff).await;
                report.push_str("\n## Self-audit (recursive development)\n");
                report.push_str(&verdict);
                report.push('\n');
            }
        }
    }

    Ok(FanoutRun {
        report,
        session,
        base,
        outcomes,
    })
}

/// Transparent recursion ledger (roadmap 5.1): when entheai develops entheai
/// (the `agy` executor), every coder turn is appended as one JSONL line to
/// `<root>/.entheai/recursion.log` — layer, role, task, and where it ended up.
/// Best-effort: a ledger write failure warns, never blocks the run.
fn log_recursive_turns(root: &Path, session: &str, outcomes: &[CoderOutcome]) {
    let layer: u32 = std::env::var("ENTHEAI_FANOUT_DEPTH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let dir = root.join(".entheai");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("recursion ledger: mkdir failed (continuing): {e}");
        return;
    }
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut lines = String::new();
    for o in outcomes {
        let line = serde_json::json!({
            "ts_ms": ts_ms,
            "session": session,
            "layer": layer,
            "index": o.index,
            "role": o.role,
            "task": o.task,
            "committed": o.committed,
            "integrated": o.integrated,
            "sealed": matches!(o.verify, VerifyStatus::Passed(_)),
        });
        lines.push_str(&line.to_string());
        lines.push('\n');
    }
    use std::io::Write;
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("recursion.log"))
        .and_then(|mut f| f.write_all(lines.as_bytes()));
    if let Err(e) = result {
        log::warn!("recursion ledger: append failed (continuing): {e}");
    }
}

/// Byte budget for each document fed to the recursive self-audit prompt.
const AUDIT_DOC_BYTES: usize = 8 * 1024;

/// Post-execution self-audit for the recursive-development path (roadmap 5.1):
/// judge the integrated diff against AGENTS.md's own rules via one extra
/// orchestrator call. Returns a report section; every failure mode degrades to
/// an honest "skipped" line — the audit can flag work, never lose it.
async fn recursive_self_audit(config: &Config, root: &Path, diff: &str) -> String {
    let agents_md = match std::fs::read_to_string(root.join("AGENTS.md")) {
        Ok(s) => s,
        Err(e) => return format!("self-audit skipped (AGENTS.md unreadable: {e})"),
    };
    let orch_model = match entheai_router::orchestrator_model(config) {
        Ok(m) => entheai_router::available_or_free(config, m),
        Err(e) => return format!("self-audit skipped (no orchestrator model: {e})"),
    };
    let system = "You are entheai auditing a change that entheai just made to ITSELF \
        (recursive development). Judge the diff ONLY against the project rules below. \
        Report: (1) any rule the diff violates, with the rule quoted; (2) any claim the \
        diff makes that the diff itself does not substantiate. Be terse and concrete. \
        If nothing is wrong, reply exactly: audit clean.";
    let user = format!(
        "## Project rules (AGENTS.md, truncated)\n{}\n\n## Integrated diff (truncated)\n{}",
        cap_str(&agents_md, AUDIT_DOC_BYTES),
        cap_str(diff, AUDIT_DOC_BYTES),
    );
    match orchestrate_once(config, &orch_model, Some(system), &user).await {
        Ok(verdict) => verdict.trim().to_string(),
        Err(e) => format!("self-audit skipped (audit call failed: {e})"),
    }
}

/// Char-boundary-safe head of `s`, ≤ `max` bytes.
fn cap_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Render a fan-out v2 run as a structured markdown-ish report. Pure/deterministic
/// (no git/LLM calls) — this IS the final answer, no extra synthesis call needed.
pub fn format_v2_report(
    task: &str,
    base: &str,
    session: &str,
    outcomes: &[CoderOutcome],
    integration: Option<&worktree::Integration>,
) -> String {
    let short_base = &base[..base.len().min(10)];
    let mut s =
        format!("# Fan-out v2 report\n\nTask: {task}\nBase: {short_base}\nSession: {session}\n");

    let mut sorted: Vec<&CoderOutcome> = outcomes.iter().collect();
    sorted.sort_by_key(|o| o.index);

    for o in sorted {
        s.push_str(&format!("\n### [{}] {}\n", o.role, o.task));
        let status = if o.timed_out {
            "timed out — not integrated".to_string()
        } else if o.oracle_rejected {
            format!("oracle rejected — left on branch {}", o.branch)
        } else if o.conflicted {
            format!("merge conflict — left on branch {}", o.branch)
        } else if !o.committed {
            "no changes".to_string()
        } else if o.integrated {
            match &o.verify {
                VerifyStatus::Passed(seal) => format!(
                    "integrated ✓ — seal {} (verify: {})",
                    &seal.seal[..12.min(seal.seal.len())],
                    seal.verify_cmd
                ),
                // Skipped (legacy lax mode) is the only other way in — say so
                // instead of letting an unverified merge look sealed.
                _ => "integrated ✓ (UNVERIFIED — [fanout].verify_required = false)".to_string(),
            }
        } else if let VerifyStatus::Failed(msg) = &o.verify {
            // Failed carries the FULL log for the trajectory sink — tail it here.
            format!(
                "changes not integrated (verify failed: {}) — left on branch {}",
                tail_chars(msg, 500),
                o.branch
            )
        } else if matches!(o.verify, VerifyStatus::Unverifiable) {
            format!(
                "changes not integrated (no verify command — set [fanout].verify or add scripts/check.sh) — left on branch {}",
                o.branch
            )
        } else {
            format!("changes on branch {} (unverified)", o.branch)
        };
        s.push_str(&format!("status: {status}\n\n"));
        s.push_str(o.output.trim());
        s.push('\n');
    }

    s.push_str("\n## Integration\n");
    match integration {
        Some(i) => {
            let files_changed = i.diff.matches("\ndiff --git ").count()
                + usize::from(i.diff.starts_with("diff --git "));
            s.push_str(&format!("Integration branch: {}\n", i.branch));
            s.push_str(&format!("Merged: {:?}\n", i.merged));
            s.push_str(&format!("Conflicted: {:?}\n", i.conflicted));
            s.push_str(&format!(
                "Diff: {files_changed} file(s) changed ({} bytes)\n",
                i.diff.len()
            ));
            s.push_str(&format!(
                "Review with: git diff {base}..{branch}  ·  checkout: git switch {branch}\n",
                branch = i.branch
            ));
        }
        None => {
            s.push_str("No changes were integrated.\n");
        }
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a git command and panic with its stderr on failure — test setup
    /// only. Mirrors `worktree::tests::git_ok`, duplicated locally since that
    /// one is private to `worktree`'s own test module.
    async fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = tokio::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .await
            .expect("failed to spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[tokio::test]
    async fn pending_workers_stops_and_reaps_untracked_ids_on_drop() {
        // Simulates the cancellation path: the async fn owning `pending` is
        // dropped mid-fan-out (e.g. the caller aborted the task) before every
        // spawned coder was joined — the still-running one must be stopped,
        // not left to run to `coder_timeout_secs` against a worktree the
        // caller's WorktreeGuard is about to remove.
        fn fake_run(index: usize) -> CoderRun {
            CoderRun {
                index,
                role: "coder".into(),
                task: "test".into(),
                branch: format!("entheai/test/coder-{index}"),
                path: std::path::PathBuf::from(format!("/tmp/entheai-test-pending-{index}")),
                output: "done".into(),
            }
        }
        let pool = WorkerPool::new(4);
        let long_running = pool.spawn(
            "coder",
            "hangs",
            std::time::Duration::from_secs(30),
            async {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                fake_run(0)
            },
        );
        let already_done = pool.spawn(
            "coder",
            "quick",
            std::time::Duration::from_secs(30),
            async { fake_run(1) },
        );
        // Let the quick one actually finish before we "join" it below.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        {
            let mut pending = PendingWorkers::new(&pool);
            pending.track(long_running);
            pending.track(already_done);
            // Normal-path join for one of them — must not be touched by drop.
            let _ = pool.join(already_done).await;
            pool.reap(already_done);
            pending.joined(already_done);
            // `pending` drops here WITHOUT `long_running` ever having been
            // joined — this is the cancellation scenario.
        }

        // `PendingWorkers::drop` calls `stop` (aborts + marks Killed) THEN `reap`
        // (removes the tracked entry) — so `None` here is the drop having acted;
        // the pre-fix bug left it tracked and `Running` forever (never joined,
        // never reaped) because nothing called `stop`/`reap` on cancellation.
        assert_eq!(
            pool.status(long_running),
            None,
            "the never-joined worker must be stopped + reaped by PendingWorkers::drop"
        );
        // Already joined+reaped by the explicit test code above — drop must be a
        // no-op for it (double-reap would be a logic error, though harmless here).
        assert_eq!(pool.status(already_done), None);
    }

    #[test]
    fn coder_status_label_distinguishes_commit_failure_from_no_changes() {
        // Regression: both used to collapse into "no changes" because the
        // caller matched only on `committed` (`.unwrap_or(false)` on a real
        // `commit_all` error looks identical to a genuine no-op commit).
        assert_eq!(
            coder_status_label(false, &VerifyStatus::Failed("commit failed: boom".into())),
            "commit failed — not integrated"
        );
        assert_eq!(
            coder_status_label(false, &VerifyStatus::NoChanges),
            "no changes"
        );
        assert_eq!(
            coder_status_label(true, &VerifyStatus::Failed("check.sh: exit 1".into())),
            "verify failed"
        );
        assert_eq!(
            coder_status_label(true, &VerifyStatus::Skipped),
            "changes committed (unverified)"
        );
        assert_eq!(
            coder_status_label(true, &VerifyStatus::Unverifiable),
            "unverifiable — not integrated"
        );
    }

    #[test]
    fn fanout_policy_follows_config() {
        let default_cfg = Config::from_toml_str("").unwrap();
        let p = fanout_policy(&default_cfg);
        // Default mode is Auto -> Exec ceiling (allows Exec, denies Network)
        assert_eq!(
            p.decide_tiered("run_shell", entheai_permission::Tier::Exec),
            entheai_permission::Decision::Allow
        );
        assert_eq!(
            p.decide_tiered("fetch", entheai_permission::Tier::Network),
            entheai_permission::Decision::Deny
        );

        let strict = Config::from_toml_str("[permission]\nfanout_auto_approve = false\n").unwrap();
        let sp = fanout_policy(&strict);
        // fanout_auto_approve = false -> Plan mode (denies Exec)
        assert_eq!(
            sp.decide_tiered("run_shell", entheai_permission::Tier::Exec),
            entheai_permission::Decision::Deny
        );
    }

    #[test]
    fn decomposed_carries_tasks() {
        let ev = FanoutEvent::Decomposed {
            tasks: vec![("coder".into(), "add x".into())],
        };
        if let FanoutEvent::Decomposed { tasks } = ev {
            assert_eq!(tasks[0].1, "add x");
        } else {
            panic!()
        }
    }

    #[test]
    fn ensure_coder_appends_when_no_coder_present() {
        // A weak model returned an explore-only plan; the guard must add a coder
        // so the v2 fan-out actually changes code instead of only analyzing it.
        let out = ensure_coder(
            vec![sub_task("explore", "analyze foo")],
            "add a doc comment",
        );
        assert_eq!(out.len(), 2);
        assert!(out
            .iter()
            .any(|s| s.role == "coder" && s.task == "add a doc comment"));
    }

    #[test]
    fn ensure_coder_is_noop_when_a_coder_is_present() {
        let plan = vec![sub_task("explore", "map"), sub_task("coder", "edit lib.rs")];
        let out = ensure_coder(plan, "whole task");
        assert_eq!(out.len(), 2, "no extra coder appended");
        assert_eq!(out.iter().filter(|s| s.role == "coder").count(), 1);
    }

    #[tokio::test]
    async fn decompose_input_is_mapped_not_raw_task() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("notes.txt"), "line one\nline two\n")
            .await
            .unwrap();
        let task = "# Fix bug\nlook at @{notes.txt}";

        let mapped = entheai_mapper::Mapper::map(dir.path(), task, &[]).await;
        let (_, user_msg) = decompose_messages(&mapped.render());

        assert!(user_msg.contains("## Section: Fix bug"));
        assert!(user_msg.contains("[file: notes.txt]"));
        assert!(user_msg.contains("### File: "));
        assert!(user_msg.contains("line one"));
        assert_ne!(user_msg, task);
    }

    #[tokio::test]
    async fn synthesis_and_fallback_inputs_are_mapped_not_raw_task() {
        // orchestrate_once never registers any tools (see its empty ToolRegistry),
        // so both the empty-decomposition fallback and the synthesis step must be
        // fed mapped.render() -- a raw `@{file}` marker would be an unresolvable
        // dead end for a model with no way to read the file itself.
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("spec.md"), "the actual spec content\n")
            .await
            .unwrap();
        let task = "@{spec.md} implement per the spec";

        let mapped = entheai_mapper::Mapper::map(dir.path(), task, &[]).await;

        // Fallback path: orchestrate_once is called directly with mapped.render().
        let fallback_user = mapped.render();
        assert!(fallback_user.contains("spec content"));
        assert!(fallback_user.contains("[file: spec.md]"));
        assert_ne!(fallback_user, task);

        // Synthesis path: synthesis_messages built from mapped content, not raw task.
        let results = vec![sub_task_result("coder", "did the work", "done")];
        let (_, synth_user_msg) = synthesis_messages(&mapped.render(), &results);
        assert!(synth_user_msg.contains("spec content"));
        assert!(synth_user_msg.contains("[file: spec.md]"));
        assert!(!synth_user_msg.contains(task));
    }

    fn sub_task_result(role: &str, task: &str, output: &str) -> SubResult {
        SubResult {
            role: role.to_string(),
            task: task.to_string(),
            output: output.to_string(),
        }
    }

    fn sub_task(role: &str, task: &str) -> SubTask {
        SubTask {
            role: role.to_string(),
            task: task.to_string(),
        }
    }

    #[test]
    fn leaf_scope_keeps_session_overrides_task_id_cwd_role() {
        let base = MemoryScope {
            session_id: "sess-1".to_string(),
            task_id: "oneshot".to_string(),
            cwd: PathBuf::from("/root"),
            role: None,
        };
        let leaf = leaf_scope(&base, "fanout-abc", 2, Path::new("/root/.wt/2"), "coder");
        assert_eq!(leaf.session_id, "sess-1");
        assert_eq!(leaf.task_id, "fanout-abc-2");
        assert_eq!(leaf.cwd, PathBuf::from("/root/.wt/2"));
        assert_eq!(leaf.role.as_deref(), Some("coder"));
    }

    #[test]
    fn parse_decomposition_parses_clean_json_array() {
        let content = r#"[{"role":"explore","task":"map the auth module"},{"role":"coder","task":"add a test"}]"#;
        let out = parse_decomposition(content, 8);
        assert_eq!(
            out,
            vec![
                sub_task("explore", "map the auth module"),
                sub_task("coder", "add a test")
            ]
        );
    }

    #[test]
    fn parse_decomposition_extracts_array_from_prose_and_fences() {
        let content = "Sure, here you go:\n```json\n[{\"role\":\"explore\",\"task\":\"map the auth module\"},{\"role\":\"coder\",\"task\":\"add a test\"}]\n```\nHope that helps!";
        let out = parse_decomposition(content, 8);
        assert_eq!(
            out,
            vec![
                sub_task("explore", "map the auth module"),
                sub_task("coder", "add a test")
            ]
        );
    }

    #[test]
    fn parse_decomposition_caps_at_max() {
        let content = r#"[
            {"role":"explore","task":"t1"},
            {"role":"coder","task":"t2"},
            {"role":"reviewer","task":"t3"},
            {"role":"test","task":"t4"},
            {"role":"docs","task":"t5"}
        ]"#;
        let out = parse_decomposition(content, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out,
            vec![sub_task("explore", "t1"), sub_task("coder", "t2")]
        );
    }

    #[test]
    fn parse_decomposition_returns_empty_for_non_json() {
        let out = parse_decomposition("I cannot decompose this task.", 8);
        assert!(out.is_empty());
    }

    #[test]
    fn parse_decomposition_filters_empty_role_or_task() {
        let content = r#"[
            {"role":"","task":"t1"},
            {"role":"coder","task":""},
            {"role":"reviewer","task":"t3"}
        ]"#;
        let out = parse_decomposition(content, 8);
        assert_eq!(out, vec![sub_task("reviewer", "t3")]);
    }

    #[test]
    fn synthesis_user_message_contains_task_roles_and_outputs() {
        let results = vec![
            SubResult {
                role: "explore".to_string(),
                task: "map the auth module".to_string(),
                output: "found 3 auth files".to_string(),
            },
            SubResult {
                role: "coder".to_string(),
                task: "add a test".to_string(),
                output: "added test_auth.rs".to_string(),
            },
        ];
        let msg = synthesis_user_message("Improve auth coverage", &results);
        assert!(msg.contains("Improve auth coverage"));
        assert!(msg.contains("explore"));
        assert!(msg.contains("coder"));
        assert!(msg.contains("found 3 auth files"));
        assert!(msg.contains("added test_auth.rs"));
    }

    fn coder_outcome(
        index: usize,
        role: &str,
        task: &str,
        branch: &str,
        committed: bool,
        verify: VerifyStatus,
        integrated: bool,
    ) -> CoderOutcome {
        CoderOutcome {
            index,
            role: role.to_string(),
            task: task.to_string(),
            branch: branch.to_string(),
            output: format!("{role} did some work"),
            committed,
            verify,
            integrated,
            conflicted: false,
            timed_out: false,
            oracle_rejected: false,
        }
    }

    #[test]
    fn format_v2_report_with_integration_shows_status_and_switch_hint() {
        let outcomes = vec![
            coder_outcome(
                0,
                "coder",
                "add a feature",
                "entheai/sess/coder-0",
                true,
                VerifyStatus::Passed(MergeSeal::compute(b"diff", b"log", "./scripts/check.sh")),
                true,
            ),
            coder_outcome(
                1,
                "test",
                "write a test",
                "entheai/sess/coder-1",
                true,
                VerifyStatus::Failed("assertion failed at line 42".to_string()),
                false,
            ),
        ];
        let integration = worktree::Integration {
            branch: "entheai/sess/integration".to_string(),
            merged: vec!["entheai/sess/coder-0".to_string()],
            conflicted: vec![],
            diff: "diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
        };

        let report = format_v2_report(
            "Ship the widget",
            "0123456789abcdef",
            "sess",
            &outcomes,
            Some(&integration),
        );

        assert!(report.contains("Ship the widget"));
        assert!(report.contains("coder"));
        assert!(report.contains("test"));
        assert!(report.contains("entheai/sess/integration"));
        assert!(report.contains("integrated"));
        // The integrated coder's line must carry its 12-hex seal prefix + provenance.
        let expected_seal = MergeSeal::compute(b"diff", b"log", "./scripts/check.sh");
        assert!(report.contains(&format!("seal {}", &expected_seal.seal[..12])));
        assert!(report.contains("verify: ./scripts/check.sh"));
        assert!(report.contains("verify failed: assertion failed at line 42"));
        assert!(report.contains("git switch entheai/sess/integration"));
    }

    #[test]
    fn merge_seal_is_deterministic_and_input_sensitive() {
        let a = MergeSeal::compute(b"diff-bytes", b"log-bytes", "cmd");
        let b = MergeSeal::compute(b"diff-bytes", b"log-bytes", "cmd");
        assert_eq!(a, b, "same inputs must yield the same seal");
        assert_eq!(a.seal.len(), 64);
        assert_eq!(a.diff_sha256.len(), 64);
        assert_eq!(a.log_sha256.len(), 64);

        let c = MergeSeal::compute(b"diff-bytes2", b"log-bytes", "cmd");
        assert_ne!(a.seal, c.seal, "a different diff must change the seal");
        let d = MergeSeal::compute(b"diff-bytes", b"log-bytes2", "cmd");
        assert_ne!(
            a.seal, d.seal,
            "a different verify log must change the seal"
        );
    }

    #[tokio::test]
    async fn verify_worktree_without_cmd_is_unverifiable_when_required_else_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let strict = verify_worktree(dir.path(), "HEAD", None, true, Duration::from_secs(5)).await;
        assert!(matches!(strict, VerifyStatus::Unverifiable));
        let lax = verify_worktree(dir.path(), "HEAD", None, false, Duration::from_secs(5)).await;
        assert!(matches!(lax, VerifyStatus::Skipped));
    }

    fn shell(cmd: &str) -> Option<VerifyCommand> {
        Some(VerifyCommand::Shell(cmd.to_string()))
    }

    #[tokio::test]
    async fn verify_worktree_failure_carries_output_tail() {
        let dir = tempfile::tempdir().unwrap();
        let status = verify_worktree(
            dir.path(),
            "HEAD",
            shell("echo boom >&2; exit 1"),
            true,
            Duration::from_secs(5),
        )
        .await;
        match status {
            VerifyStatus::Failed(msg) => assert!(msg.contains("boom")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn verify_worktree_times_out_instead_of_hanging_forever() {
        let dir = tempfile::tempdir().unwrap();
        let status = verify_worktree(
            dir.path(),
            "HEAD",
            shell("sleep 5"),
            true,
            Duration::from_millis(100),
        )
        .await;
        match status {
            VerifyStatus::Failed(msg) => assert!(msg.contains("timed out"), "{msg}"),
            other => panic!("expected Failed(timed out), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn verify_worktree_runs_a_pinned_script_via_stdin() {
        // The path a coder can't tamper with: the base-commit content of
        // ./scripts/check.sh, executed via `sh -s < content` instead of the
        // worktree's own (possibly coder-edited) copy.
        let dir = tempfile::tempdir().unwrap();
        let status = verify_worktree(
            dir.path(),
            "HEAD",
            Some(VerifyCommand::PinnedScript(b"echo pinned-ok\n".to_vec())),
            true,
            Duration::from_secs(5),
        )
        .await;
        match status {
            VerifyStatus::Passed(_) => {}
            other => panic!("expected Passed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_verify_command_pins_auto_detected_script_to_base() {
        // A repo with ./scripts/check.sh committed at `base`, then the
        // WORKTREE's copy tampered with (as a malicious/careless coder would):
        // resolve_verify_command must still hand back the base content, not
        // whatever is on disk right now.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git_ok(root, &["init", "-b", "main"]).await;
        git_ok(root, &["config", "user.email", "t@t"]).await;
        git_ok(root, &["config", "user.name", "t"]).await;
        tokio::fs::create_dir_all(root.join("scripts"))
            .await
            .unwrap();
        tokio::fs::write(root.join("scripts/check.sh"), "echo real-gate\n")
            .await
            .unwrap();
        git_ok(root, &["add", "-A"]).await;
        git_ok(root, &["commit", "-m", "init"]).await;
        let base = git_ok(root, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_string();

        // Tamper with the WORKING TREE's copy (what a coder's worktree would have).
        tokio::fs::write(root.join("scripts/check.sh"), "exit 0 # tampered\n")
            .await
            .unwrap();

        let config = entheai_config::Config::from_toml_str("").unwrap();
        let resolved = resolve_verify_command(&config, root, &base)
            .await
            .expect("auto-detected script must resolve");
        match resolved {
            VerifyCommand::PinnedScript(content) => {
                assert_eq!(
                    String::from_utf8_lossy(&content),
                    "echo real-gate\n",
                    "must be the BASE commit's content, not the tampered worktree copy"
                );
            }
            VerifyCommand::Shell(cmd) => {
                panic!("expected the auto-detected script to be pinned, got Shell({cmd:?})")
            }
        }
    }

    #[tokio::test]
    async fn resolve_verify_command_trusts_explicit_operator_config() {
        // [fanout].verify is an operator-chosen command, not the coder-writable
        // auto-detected script — never pinned/rewritten.
        let dir = tempfile::tempdir().unwrap();
        let config =
            entheai_config::Config::from_toml_str("[fanout]\nverify = \"cargo test\"\n").unwrap();
        let resolved = resolve_verify_command(&config, dir.path(), "HEAD").await;
        assert!(matches!(resolved, Some(VerifyCommand::Shell(cmd)) if cmd == "cargo test"));
    }

    /// Capture-only sink: records every (meta, trace) it is fed.
    struct CaptureSink(std::sync::Mutex<Vec<(serde_json::Value, String)>>);

    #[async_trait::async_trait]
    impl TrajectorySink for CaptureSink {
        async fn ingest_failure(&self, meta: serde_json::Value, trace: &str) {
            self.0.lock().unwrap().push((meta, trace.to_string()));
        }
    }

    #[tokio::test]
    async fn failed_verify_feeds_the_trajectory_sink_with_full_log() {
        // The seam run_fanout uses: a Failed verify + a sink → one ingest with
        // the FULL traceback (not the display tail).
        let dir = tempfile::tempdir().unwrap();
        let long_msg = "x".repeat(2000);
        let status = verify_worktree(
            dir.path(),
            "HEAD",
            shell(&format!("echo '{long_msg}' >&2; exit 1")),
            true,
            Duration::from_secs(5),
        )
        .await;
        let sink = CaptureSink(std::sync::Mutex::new(Vec::new()));
        if let VerifyStatus::Failed(trace) = &status {
            let meta = serde_json::json!({
                "source": "fanout-verify",
                "role": "coder",
                "branch": "entheai/sess/coder-0",
            });
            sink.ingest_failure(meta, trace).await;
        }
        let captured = sink.0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let (meta, trace) = &captured[0];
        assert_eq!(meta["source"], "fanout-verify");
        // Full log survives (>500 chars — the display tail must not truncate it).
        assert!(trace.chars().count() > 1500, "sink got a tail, not the log");
    }

    #[test]
    fn format_v2_report_unverifiable_coder_is_left_on_branch_with_remedy() {
        let outcomes = vec![coder_outcome(
            0,
            "coder",
            "add a feature",
            "entheai/sess/coder-0",
            true,
            VerifyStatus::Unverifiable,
            false,
        )];
        let report = format_v2_report("Ship it", "0123456789abcdef", "sess", &outcomes, None);
        assert!(report.contains("no verify command"));
        assert!(report.contains("left on branch entheai/sess/coder-0"));
        assert!(report.contains("scripts/check.sh"));
    }

    #[test]
    fn cap_str_is_char_boundary_safe() {
        assert_eq!(cap_str("hello", 10), "hello");
        assert_eq!(cap_str("hello", 3), "hel");
        // "🧊" is 4 bytes; a 5-byte cap must not split the second emoji.
        assert_eq!(cap_str("🧊🧊", 5), "🧊");
    }

    #[test]
    fn recursion_ledger_appends_one_jsonl_line_per_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let outcomes = vec![
            coder_outcome(
                0,
                "coder",
                "add a feature",
                "entheai/sess/coder-0",
                true,
                VerifyStatus::Passed(MergeSeal::compute(b"d", b"l", "cmd")),
                true,
            ),
            coder_outcome(
                1,
                "test",
                "write tests",
                "entheai/sess/coder-1",
                true,
                VerifyStatus::Failed("boom".into()),
                false,
            ),
        ];
        log_recursive_turns(dir.path(), "sess-a", &outcomes);
        log_recursive_turns(dir.path(), "sess-b", &outcomes[..1]);
        let raw = std::fs::read_to_string(dir.path().join(".entheai/recursion.log")).unwrap();
        let lines: Vec<serde_json::Value> = raw
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "append-only: 2 + 1 turns");
        assert_eq!(lines[0]["session"], "sess-a");
        assert_eq!(lines[0]["role"], "coder");
        assert_eq!(lines[0]["sealed"], true);
        assert_eq!(lines[1]["sealed"], false);
        assert_eq!(lines[1]["integrated"], false);
        assert_eq!(lines[2]["session"], "sess-b");
    }

    #[tokio::test]
    async fn recursive_self_audit_degrades_honestly_without_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = entheai_config::Config::from_toml_str("").unwrap();
        let verdict = recursive_self_audit(&cfg, dir.path(), "diff --git a b").await;
        assert!(
            verdict.starts_with("self-audit skipped"),
            "no AGENTS.md must yield an honest skip, got: {verdict}"
        );
    }

    #[test]
    fn format_v2_report_lax_integration_is_loudly_unverified() {
        let outcomes = vec![coder_outcome(
            0,
            "coder",
            "add a feature",
            "entheai/sess/coder-0",
            true,
            VerifyStatus::Skipped,
            true,
        )];
        let report = format_v2_report("Ship it", "0123456789abcdef", "sess", &outcomes, None);
        assert!(report.contains("UNVERIFIED"));
        assert!(report.contains("verify_required = false"));
    }

    #[test]
    fn format_v2_report_without_integration_says_nothing_was_integrated() {
        let outcomes = vec![coder_outcome(
            0,
            "coder",
            "add a feature",
            "entheai/sess/coder-0",
            false,
            VerifyStatus::NoChanges,
            false,
        )];

        let report = format_v2_report(
            "Ship the widget",
            "0123456789abcdef",
            "sess",
            &outcomes,
            None,
        );

        assert!(report.contains("No changes were integrated."));
    }

    #[test]
    fn format_v2_report_marks_timed_out_run_as_not_integrated() {
        // Bug 2: a timed-out/killed coder must be reported as failed and must NOT
        // show up as integrated, even though nothing about verify changed.
        let outcomes = vec![CoderOutcome {
            index: 0,
            role: "coder".to_string(),
            task: "long task".to_string(),
            branch: "entheai/sess/coder-0".to_string(),
            output: "error: coder timed out".to_string(),
            committed: false,
            verify: VerifyStatus::NoChanges,
            integrated: false,
            conflicted: false,
            timed_out: true,
            oracle_rejected: false,
        }];

        let report = format_v2_report("Do the thing", "0123456789abcdef", "sess", &outcomes, None);

        assert!(
            report.contains("timed out — not integrated"),
            "report:\n{report}"
        );
        assert!(!report.contains("integrated ✓"), "report:\n{report}");
        assert!(report.contains("No changes were integrated."));
    }

    #[test]
    fn format_v2_report_marks_oracle_rejected_coder_as_left_on_branch() {
        // An Oracle-rejected coder (gate=block, high-confidence Reject) must
        // report as not integrated and left on its branch, like a conflict.
        let outcomes = vec![CoderOutcome {
            index: 0,
            role: "coder".to_string(),
            task: "risky task".to_string(),
            branch: "entheai/sess/coder-0".to_string(),
            output: "done".to_string(),
            committed: true,
            verify: VerifyStatus::Passed(MergeSeal {
                diff_sha256: "b".repeat(64),
                log_sha256: "c".repeat(64),
                seal: "a".repeat(64),
                verify_cmd: "cargo test".to_string(),
            }),
            integrated: false,
            conflicted: false,
            timed_out: false,
            oracle_rejected: true,
        }];

        let report = format_v2_report("Do the thing", "0123456789abcdef", "sess", &outcomes, None);

        assert!(
            report.contains("oracle rejected — left on branch entheai/sess/coder-0"),
            "report:\n{report}"
        );
        assert!(!report.contains("integrated ✓"), "report:\n{report}");
    }

    #[test]
    fn ceiling_policy_denies_above_the_parent_ceiling() {
        use entheai_permission::{Decision, Tier};
        let p = ceiling_policy(entheai_permission::Mode::Plan); // Read ceiling
        assert_eq!(p.decide_tiered("read_file", Tier::Read), Decision::Allow);
        assert_eq!(p.decide_tiered("run_shell", Tier::Exec), Decision::Deny);
        let a = ceiling_policy(entheai_permission::Mode::Auto); // Exec ceiling
        assert_eq!(a.decide_tiered("run_shell", Tier::Exec), Decision::Allow);
        assert_eq!(a.decide_tiered("fetch", Tier::Network), Decision::Deny);
    }
}
