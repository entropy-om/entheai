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
//! each other. Each coder's worktree is committed, optionally verified (
//! `[fanout].verify`), and — if it committed and verified clean — integrated
//! onto a fresh integration branch. Returns a structured report instead of an
//! extra synthesis LLM call.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use entheai_config::Config;
use entheai_providers::ChatMessage;
use futures::stream::{self, StreamExt};
use serde::Deserialize;

pub mod pool;
pub mod worktree;

pub use pool::{WorkerId, WorkerPool, WorkerStatus, WorkerSummary};

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
/// this is never actually consulted; it exists to satisfy `run_task`'s signature.
struct AutoAllow;
#[async_trait::async_trait]
impl entheai_permission::Prompter for AutoAllow {
    async fn confirm(&mut self, _tool: &str, _args: &str) -> entheai_permission::Grant {
        entheai_permission::Grant::Allow
    }
}

/// Build the fan-out sub-agent/coder permission policy from config. Fan-out
/// runs are unattended, so approval is driven by `[permission].fanout_auto_approve`
/// (default true) plus the shared `[permission].allowlist`.
fn fanout_policy(config: &Config) -> entheai_permission::Policy {
    entheai_permission::Policy::new(
        config.permission.fanout_auto_approve,
        config.permission.allowlist.clone(),
    )
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

fn decompose_messages(task: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(DECOMPOSE_SYSTEM),
        ChatMessage::user(task),
    ]
}

fn subagent_messages(role: &str, task: &str) -> Vec<ChatMessage> {
    let sys = format!(
        "You are a `{role}` sub-agent in a fan-out coding agent. Use read_file/search to \
         investigate, then report your findings concisely. You cannot modify files."
    );
    vec![ChatMessage::system(sys), ChatMessage::user(task)]
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

fn synthesis_messages(task: &str, results: &[SubResult]) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(
            "You are the orchestrator. Synthesize the sub-agent results into one clear final answer.",
        ),
        ChatMessage::user(synthesis_user_message(task, results)),
    ]
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

/// Run the orchestrator model once (empty registry = a single completion).
async fn orchestrate_once(
    config: &Config,
    model_id: &str,
    messages: Vec<ChatMessage>,
) -> anyhow::Result<String> {
    // Prepend the orchestrator identity prompt unless the caller already set one.
    let mut messages = messages;
    if !messages
        .first()
        .map(|m| m.role == "system")
        .unwrap_or(false)
    {
        messages.insert(
            0,
            ChatMessage::system(entheai_router::orchestrator_system_prompt(config)),
        );
    }
    let agent = entheai_router::build_agent(model_id, config)?;
    let registry = entheai_tools::ToolRegistry::new();
    let policy = fanout_policy(config);
    let mut prompter = AutoAllow;
    let out = agent
        .run_task(messages, &registry, &policy, &mut prompter, None)
        .await?;
    Ok(out)
}

/// Run one sub-agent to completion. Never returns Err — a failure is captured as
/// the sub-result's `output` so one bad sub-agent doesn't sink the whole batch.
async fn run_subagent(config: &Config, root: &Path, st: SubTask) -> SubResult {
    let output = async {
        let model_id = entheai_router::model_for_role(config, &st.role)?;
        let agent = entheai_router::build_agent(&model_id, config)?;
        let registry = read_only_registry(root);
        let policy = fanout_policy(config);
        let mut prompter = AutoAllow;
        let out = agent
            .run_task(
                subagent_messages(&st.role, &st.task),
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await?;
        Ok::<String, anyhow::Error>(out)
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
async fn run_fanout_readonly(config: &Config, root: &Path, task: &str) -> anyhow::Result<String> {
    let orch_model = entheai_router::orchestrator_model(config)?;

    // 1. Map + decompose.
    let mapped = entheai_mapper::Mapper::map(root, task, &[]).await;
    let raw = orchestrate_once(config, &orch_model, decompose_messages(&mapped.render())).await?;
    let max_par = config.router.max_parallel.max(1);
    let subtasks = parse_decomposition(&raw, max_par);

    // Fallback: couldn't decompose → just run the task once on the orchestrator.
    // Uses `mapped.render()`, not raw `task`: `orchestrate_once` never registers any
    // tools, so a raw `@{file}` marker here would be a dead end the model can't resolve.
    if subtasks.is_empty() {
        return orchestrate_once(
            config,
            &orch_model,
            vec![ChatMessage::user(mapped.render())],
        )
        .await;
    }

    // 2. Fan out, bounded by max_parallel.
    let results: Vec<SubResult> = stream::iter(subtasks)
        .map(|st| run_subagent(config, root, st))
        .buffer_unordered(max_par)
        .collect()
        .await;

    // 3. Synthesize. Same reasoning as the fallback above: the synthesis call has no
    // tool access either, so it needs the resolved file content, not a raw marker.
    orchestrate_once(
        config,
        &orch_model,
        synthesis_messages(&mapped.render(), &results),
    )
    .await
}

/// Full (read/write/shell/search) tool set for a coder sub-agent, rooted at its
/// own isolated worktree — safe to run in parallel because each coder's `root`
/// is a distinct `git worktree` checkout, not the shared process cwd.
fn write_registry(root: &Path) -> entheai_tools::ToolRegistry {
    let mut r = entheai_tools::ToolRegistry::new();
    r.register(Box::new(entheai_tools::fs::ReadFile::new(
        root.to_path_buf(),
    )));
    r.register(Box::new(entheai_tools::fs::WriteFile::new(
        root.to_path_buf(),
    )));
    r.register(Box::new(entheai_tools::fs::EditFile::new(
        root.to_path_buf(),
    )));
    r.register(Box::new(entheai_tools::shell::RunShell::new(
        root.to_path_buf(),
    )));
    r.register(Box::new(entheai_tools::search::Search::new(
        root.to_path_buf(),
    )));
    r
}

fn coder_messages(role: &str, task: &str) -> Vec<ChatMessage> {
    let sys = format!(
        "You are a `{role}` sub-agent working in an ISOLATED git worktree. Make the necessary \
         code changes with write_file/run_shell to accomplish your task. Keep changes minimal \
         and focused."
    );
    vec![ChatMessage::system(sys), ChatMessage::user(task)]
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
        let model_id = entheai_router::model_for_role(config, role)?;
        let agent = entheai_router::build_agent(&model_id, config)?;
        let registry = write_registry(worktree_path);
        let policy = fanout_policy(config);
        let mut prompter = AutoAllow;
        let out = agent
            .run_task(
                coder_messages(role, task),
                &registry,
                &policy,
                &mut prompter,
                None,
            )
            .await?;
        Ok::<String, anyhow::Error>(out)
    }
    .await
    .unwrap_or_else(|e| format!("error: coder failed: {e}"))
}

/// Run one coder sub-agent to completion inside its own worktree. Never returns
/// Err — a failure is captured as the run's `output`, mirroring [`run_subagent`],
/// so one bad coder doesn't sink the whole fan-out.
async fn run_coder(
    config: Arc<Config>,
    wt: worktree::Worktree,
    st: SubTask,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
) -> CoderRun {
    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::CoderStarted {
            index: wt.index,
            role: st.role.clone(),
            task: st.task.clone(),
        });
    }
    let output = run_coder_once(&config, &st.role, &st.task, &wt.path).await;
    CoderRun {
        index: wt.index,
        role: st.role,
        task: st.task,
        branch: wt.branch,
        path: wt.path,
        output,
    }
}

/// Outcome of running an optional `[fanout].verify` command in a coder's worktree.
#[derive(Debug, Clone)]
pub enum VerifyStatus {
    /// The coder made no commit — nothing to verify.
    NoChanges,
    /// No `[fanout].verify` command configured.
    Skipped,
    /// The verify command exited successfully.
    Passed,
    /// The verify command failed; carries the tail of its combined stderr+stdout.
    Failed(String),
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

/// Run `cmd` (if any) in `path` to decide whether a coder's changes are safe to
/// integrate. `None` skips verification entirely (changes are integrated as-is).
async fn verify_worktree(path: &Path, cmd: Option<&str>) -> VerifyStatus {
    let Some(cmd) = cmd else {
        return VerifyStatus::Skipped;
    };
    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(path)
        .output()
        .await
    {
        Ok(output) if output.status.success() => VerifyStatus::Passed,
        Ok(output) => {
            let mut combined = String::from_utf8_lossy(&output.stderr).into_owned();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            VerifyStatus::Failed(tail_chars(&combined, 500))
        }
        Err(e) => VerifyStatus::Failed(format!("failed to spawn verify command `{cmd}`: {e}")),
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
}

/// Fan-out entrypoint (v2): decompose → one isolated `git worktree` + coder
/// sub-agent per sub-task, run in parallel (≤ `router.max_parallel`) → commit +
/// optionally verify each worktree → integrate the eligible branches onto a
/// fresh integration branch → return a structured report (no extra LLM call).
///
/// Falls back to the read-only v1 fan-out ([`run_fanout_readonly`]) when `root`
/// isn't a git repo (isolated worktrees require one).
// TODO(@rahulmranga): memory-v1 Task 10 — give fan-out leaves the shared memory.
// Add a trailing `memory: Option<entheai_memory::SharedMemory>` param to `run_fanout`,
// `run_fanout_readonly`, `run_subagent`, and `run_coder` (an ADDITIONAL arg *after*
// `pool` — the signature drifted from the plan), add `entheai-memory = { path = "../memory" }`
// to crates/orchestrator/Cargo.toml, build a per-leaf `MemoryRuntime`, and swap each
// leaf's `run_task` → `run_task_with_memory`. The orchestrate_once decompose/synthesis
// meta-calls stay on plain `run_task`. Verbatim recipe:
// docs/superpowers/plans/2026-07-19-entheai-memory-v1.md → "Task 10".
pub async fn run_fanout(
    config: &Config,
    root: &Path,
    task: &str,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    pool: Arc<WorkerPool>,
) -> anyhow::Result<String> {
    if !worktree::is_git_repo(root).await {
        if let Some(tx) = &events {
            let _ = tx.send(FanoutEvent::Fallback);
        }
        let out = run_fanout_readonly(config, root, task).await?;
        return Ok(format!("(not a git repo — read-only fan-out)\n\n{out}"));
    }

    let orch_model = entheai_router::orchestrator_model(config)?;
    let max_par = config.router.max_parallel.max(1);

    // 1. Map + decompose.
    let mapped = entheai_mapper::Mapper::map(root, task, &[]).await;
    let raw = orchestrate_once(config, &orch_model, decompose_messages(&mapped.render())).await?;
    let subtasks = parse_decomposition(&raw, max_par);

    // Fallback: couldn't decompose → just run the task once on the orchestrator.
    // Uses `mapped.render()`, not raw `task`: `orchestrate_once` never registers any
    // tools, so a raw `@{file}` marker here would be a dead end the model can't resolve.
    if subtasks.is_empty() {
        return orchestrate_once(
            config,
            &orch_model,
            vec![ChatMessage::user(mapped.render())],
        )
        .await;
    }
    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::Decomposed {
            tasks: subtasks
                .iter()
                .map(|s| (s.role.clone(), s.task.clone()))
                .collect(),
        });
    }

    let session = uuid::Uuid::new_v4().simple().to_string();
    let base = worktree::resolve_base(root, "HEAD").await?;
    let wt_pool = worktree::WorktreePool::new(root, &session, &base).await?;

    // 2. Create one worktree per sub-task, sequentially (git worktree creation
    // isn't safe to parallelize against the same root repo).
    let mut wts: Vec<(worktree::Worktree, SubTask)> = Vec::with_capacity(subtasks.len());
    for (i, st) in subtasks.into_iter().enumerate() {
        let wt = wt_pool.create(i).await?;
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
            run_coder(
                Arc::clone(&config_arc),
                wt.clone(),
                st.clone(),
                events.clone(),
            ),
        );
        worker_ids.push((id, wt, st));
    }

    let mut runs: Vec<CoderRun> = Vec::with_capacity(worker_ids.len());
    for (id, wt, st) in worker_ids {
        let run = match pool.join(id).await {
            Some(run) => run,
            None => {
                let reason = match pool.status(id) {
                    Some(WorkerStatus::Killed) => "coder killed (stopped via /workers)",
                    _ => "coder timed out",
                };
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
        runs.push(run);
    }

    // 4. Commit + verify each worktree, sequentially (each is a separate git
    // invocation against a distinct worktree, but keeping this sequential keeps
    // output/ordering simple and avoids piling up concurrent `sh -c` verify runs).
    let mut outcomes: Vec<CoderOutcome> = Vec::with_capacity(runs.len());
    let mut eligible_branches: Vec<String> = Vec::new();
    for run in runs {
        let committed = worktree::commit_all(
            &run.path,
            &format!("entheai fan-out [{}]: {}", run.role, run.task),
        )
        .await
        .unwrap_or(false);
        let verify = if committed {
            verify_worktree(&run.path, config.fanout.verify.as_deref()).await
        } else {
            VerifyStatus::NoChanges
        };
        if let Some(tx) = &events {
            let status = if !committed {
                "no changes"
            } else {
                match &verify {
                    VerifyStatus::Failed(_) => "verify failed",
                    VerifyStatus::Passed => "verified",
                    VerifyStatus::Skipped => "changes committed (unverified)",
                    VerifyStatus::NoChanges => "no changes",
                }
            };
            let _ = tx.send(FanoutEvent::CoderFinished {
                index: run.index,
                committed,
                status: status.to_string(),
            });
        }
        let integrated =
            committed && matches!(verify, VerifyStatus::Skipped | VerifyStatus::Passed);
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
        });
    }

    // 5. Integrate the eligible branches onto a fresh integration branch. A
    // branch can still conflict here even though it verified clean in
    // isolation (two coders touching the same lines) — reflect that in the
    // per-branch status, overriding `integrated` for anything that conflicted.
    let integration = if eligible_branches.is_empty() {
        None
    } else {
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
        for outcome in outcomes.iter_mut() {
            if integration.conflicted.contains(&outcome.branch) {
                outcome.integrated = false;
                outcome.conflicted = true;
            }
        }
        Some(integration)
    };

    // 6. Cleanup worktrees (best-effort; keep the integration branch for review).
    for (wt, _) in &wts {
        let _ = wt_pool.remove(wt).await;
    }

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

    Ok(format_v2_report(
        task,
        &base,
        &session,
        &outcomes,
        integration.as_ref(),
    ))
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
        let status = if o.conflicted {
            format!("merge conflict — left on branch {}", o.branch)
        } else if !o.committed {
            "no changes".to_string()
        } else if o.integrated {
            "integrated ✓".to_string()
        } else if let VerifyStatus::Failed(msg) = &o.verify {
            format!("changes not integrated (verify failed: {msg})")
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

    #[test]
    fn fanout_policy_follows_config() {
        let yolo = Config::from_toml_str("").unwrap(); // fanout_auto_approve defaults true
        assert!(fanout_policy(&yolo).is_yolo());
        let strict = Config::from_toml_str("[permission]\nfanout_auto_approve = false\n").unwrap();
        assert!(!fanout_policy(&strict).is_yolo());
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

    #[tokio::test]
    async fn decompose_input_is_mapped_not_raw_task() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("notes.txt"), "line one\nline two\n")
            .await
            .unwrap();
        let task = "# Fix bug\nlook at @{notes.txt}";

        let mapped = entheai_mapper::Mapper::map(dir.path(), task, &[]).await;
        let messages = decompose_messages(&mapped.render());

        let user_msg = messages
            .iter()
            .find(|m| m.role == "user")
            .expect("user message");
        assert!(user_msg.content.contains("## Section: Fix bug"));
        assert!(user_msg.content.contains("[file: notes.txt]"));
        assert!(user_msg.content.contains("### File: "));
        assert!(user_msg.content.contains("line one"));
        assert_ne!(user_msg.content, task);
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

        // Fallback path: a single-shot ChatMessage::user built from mapped content.
        let fallback_msg = ChatMessage::user(mapped.render());
        assert!(fallback_msg.content.contains("spec content"));
        assert!(fallback_msg.content.contains("[file: spec.md]"));
        assert_ne!(fallback_msg.content, task);

        // Synthesis path: synthesis_messages built from mapped content, not raw task.
        let results = vec![sub_task_result("coder", "did the work", "done")];
        let synth_messages = synthesis_messages(&mapped.render(), &results);
        let synth_user_msg = synth_messages
            .iter()
            .find(|m| m.role == "user")
            .expect("user message");
        assert!(synth_user_msg.content.contains("spec content"));
        assert!(synth_user_msg.content.contains("[file: spec.md]"));
        assert!(!synth_user_msg.content.contains(task));
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
                VerifyStatus::Passed,
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
        assert!(report.contains("verify failed: assertion failed at line 42"));
        assert!(report.contains("git switch entheai/sess/integration"));
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
}
