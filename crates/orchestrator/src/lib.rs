//! Fan-out orchestration: an orchestrator model decomposes a task into
//! independent sub-tasks, sub-agents (model-matched per role) run them in
//! parallel, and the orchestrator synthesizes a final answer.
//!
//! v1 scope: sub-agents share the process cwd and get a READ-ONLY tool set
//! (`read_file` + `search`) — safe for parallel exploration/analysis. Parallel
//! *writers* (coders) in isolated git worktrees are a v2 follow-up; keep this
//! module's seams ready for that (per-role model routing is already here).

use std::path::Path;

use entheai_config::Config;
use entheai_providers::ChatMessage;
use futures::stream::{self, StreamExt};
use serde::Deserialize;

pub mod worktree;

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
    async fn confirm(&mut self, _tool: &str, _args: &str) -> bool {
        true
    }
}

fn yolo() -> entheai_permission::Policy {
    entheai_permission::Policy {
        yolo: true,
        allowlist: vec![],
    }
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
    let agent = entheai_router::build_agent(model_id, config)?;
    let registry = entheai_tools::ToolRegistry::new();
    let policy = yolo();
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
        let policy = yolo();
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

/// Fan-out entrypoint: decompose → parallel sub-agents (≤ router.max_parallel) → synthesize.
/// Falls back to a single orchestrator run if decomposition yields no sub-tasks.
pub async fn run_fanout(config: &Config, root: &Path, task: &str) -> anyhow::Result<String> {
    let orch_model = entheai_router::orchestrator_model(config)?;

    // 1. Decompose.
    let raw = orchestrate_once(config, &orch_model, decompose_messages(task)).await?;
    let max_par = config.router.max_parallel.max(1);
    let subtasks = parse_decomposition(&raw, max_par);

    // Fallback: couldn't decompose → just run the task once on the orchestrator.
    if subtasks.is_empty() {
        return orchestrate_once(config, &orch_model, vec![ChatMessage::user(task)]).await;
    }

    // 2. Fan out, bounded by max_parallel.
    let results: Vec<SubResult> = stream::iter(subtasks)
        .map(|st| run_subagent(config, root, st))
        .buffer_unordered(max_par)
        .collect()
        .await;

    // 3. Synthesize.
    orchestrate_once(config, &orch_model, synthesis_messages(task, &results)).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
