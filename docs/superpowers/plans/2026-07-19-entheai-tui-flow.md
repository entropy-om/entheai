# entheai TUI Flow — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the entheai TUI a single-window flow: a live plan pane, an enriched progress line (verb · elapsed · token tally), an `allow-for-session` permission flow, and an idle-frugal render loop.

**Architecture:** A 3-way permission grant (`Deny`/`Allow`/`AllowSession`) backed by a mutable per-run allowlist inside `Policy`; a `todo` tool the model calls (the TUI reads its plan off the existing `ToolStarted` event) plus fan-out auto-population; a new plan-pane region in the ratatui layout; and a dirty-flag render loop with cached wrapped lines.

**Tech Stack:** Rust, `ratatui` 0.29, `crossterm` 0.28, `tokio`, `serde_json`.

**Spec:** `docs/superpowers/specs/2026-07-19-entheai-tui-transparency-design.md`.

**⚠️ Repo hazard:** multi-session shared checkout with a `reset --hard origin/main` automation. Every task: **scoped `git add <paths>` (never `-A`), then `git push origin main` immediately** after the commit. On non-fast-forward: `git fetch origin && git rebase origin/main`, re-push; abort+report on out-of-scope conflicts.

## Shared types (defined once, used across tasks)

- `entheai_permission::Grant { Deny, Allow, AllowSession }` — the prompter's 3-way answer (Task 1).
- `entheai_permission::Policy` gains `session: Arc<Mutex<HashSet<String>>>` + `Policy::new(yolo, allowlist)` + `grant_session(&self, tool)` (Task 2).
- `entheai_tools::todo::{TodoStatus, TodoItem, parse_todos, TodoTool}` (Task 4).
- `entheai_orchestrator::FanoutEvent::Decomposed { tasks: Vec<(String, String)> }` (Task 5).

## File structure

```
crates/permission/src/lib.rs   # Grant, Policy.session + new()/grant_session, Prompter->Grant, StdinPrompter 3-way
crates/tools/src/todo.rs (new) # TodoStatus/TodoItem/parse_todos/TodoTool
crates/tools/src/lib.rs        # + pub mod todo;
crates/core/src/lib.rs         # dispatch_call: Ask -> Grant -> allow/deny/grant_session; tests
crates/orchestrator/src/lib.rs # FanoutEvent::Decomposed{tasks}
bin/entheai/src/main.rs        # register TodoTool + advertise it; Policy::new
crates/tui/src/lib.rs          # plan pane, token/verb progress line, 3-option modal, line-cache + dirty render
```

---

### Task 1: 3-way permission grant (`Grant`) + prompter returns it

**Files:** Modify `crates/permission/src/lib.rs`

- [ ] **Step 1: Failing test** — append to `crates/permission/src/lib.rs` tests:
```rust
    #[test]
    fn grant_has_three_variants() {
        let _ = (Grant::Deny, Grant::Allow, Grant::AllowSession);
        assert_ne!(Grant::Allow, Grant::Deny);
    }
```
- [ ] **Step 2: Run — FAIL** (`Grant` undefined): `cargo test -p entheai-permission grant_has_three`
- [ ] **Step 3: Implement** — add the enum + change `Prompter::confirm` to return it:
```rust
/// A user's answer to a permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Deny this call.
    Deny,
    /// Allow just this one call.
    Allow,
    /// Allow this tool for the rest of the session (no more prompts for it).
    AllowSession,
}
```
Change the trait method: `async fn confirm(&mut self, tool_name: &str, args_summary: &str) -> Grant;` (was `-> bool`).
Update `StdinPrompter::confirm` to prompt three ways and map the answer:
```rust
    async fn confirm(&mut self, tool_name: &str, args_summary: &str) -> Grant {
        use std::io::Write;
        eprint!("allow {tool_name}({args_summary})? [y]es / [n]o / [a]llow for session ");
        let _ = std::io::stderr().flush();
        tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                return Grant::Deny;
            }
            match line.trim().to_lowercase().as_str() {
                "y" | "yes" => Grant::Allow,
                "a" | "allow" | "s" | "session" => Grant::AllowSession,
                _ => Grant::Deny,
            }
        })
        .await
        .unwrap_or(Grant::Deny)
    }
```
Update the permission crate's own tests that implement `Prompter` (if any) to return `Grant`.
- [ ] **Step 4: Run — PASS.** `cargo test -p entheai-permission`
- [ ] **Step 5: Commit + push**
```bash
git add crates/permission/src/lib.rs
git commit -m "feat(permission): 3-way Grant (deny/allow/allow-for-session); Prompter returns it"
git push origin main
```

---

### Task 2: mutable per-session allowlist in `Policy`

**Files:** Modify `crates/permission/src/lib.rs`, and every `Policy { ... }` constructor: `bin/entheai/src/main.rs`, `crates/orchestrator/src/lib.rs`, `crates/core/src/lib.rs` (tests), `crates/tui/src/lib.rs` (if any).

- [ ] **Step 1: Failing test** — `crates/permission/src/lib.rs`:
```rust
    #[test]
    fn session_grant_makes_decide_allow() {
        let p = Policy::new(false, vec![]);
        assert_eq!(p.decide("run_shell"), Decision::Ask);
        p.grant_session("run_shell");
        assert_eq!(p.decide("run_shell"), Decision::Allow);
        assert_eq!(p.decide("write_file"), Decision::Ask); // unaffected
    }
```
- [ ] **Step 2: Run — FAIL.** `cargo test -p entheai-permission session_grant`
- [ ] **Step 3: Implement** — add the field + methods:
```rust
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub yolo: bool,
    pub allowlist: Vec<String>,
    /// Tools granted "for this session" at runtime (via `Grant::AllowSession`).
    session: Arc<Mutex<HashSet<String>>>,
}

impl Policy {
    pub fn new(yolo: bool, allowlist: Vec<String>) -> Self {
        Self { yolo, allowlist, session: Arc::new(Mutex::new(HashSet::new())) }
    }
    /// Grant a tool for the rest of the session; subsequent `decide` calls Allow it.
    pub fn grant_session(&self, tool_name: &str) {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tool_name.to_string());
    }
    pub fn decide(&self, tool_name: &str) -> Decision {
        if self.yolo || self.allowlist.iter().any(|t| t == tool_name) {
            return Decision::Allow;
        }
        if self.session.lock().unwrap_or_else(|e| e.into_inner()).contains(tool_name) {
            return Decision::Allow;
        }
        Decision::Ask
    }
}
```
Then fix every struct-literal construction `Policy { yolo: X, allowlist: Y }` → `Policy::new(X, Y)`. Grep first: `grep -rn "Policy {" crates/ bin/` — update each (main.rs, orchestrator `yolo()`, core tests, any tui). The `session` field is private, so literals won't compile until switched to `Policy::new`.
- [ ] **Step 4: Run — PASS + workspace compiles.** `cargo test -p entheai-permission session_grant && cargo build --workspace`
- [ ] **Step 5: Commit + push**
```bash
git add crates/permission/src/lib.rs bin/entheai/src/main.rs crates/orchestrator/src/lib.rs crates/core/src/lib.rs
git commit -m "feat(permission): mutable per-session allowlist (Policy::new/grant_session/decide)"
git push origin main
```

---

### Task 3: `dispatch_call` honors the 3-way grant

**Files:** Modify `crates/core/src/lib.rs`

- [ ] **Step 1: Failing test** — add to `crates/core/src/lib.rs` tests (reuse `RecordingProvider`/`EchoTool`; add an `AllowSessionPrompter`). The key behavior: a tool answered `AllowSession` on its first call is **not** prompted on its second call. Script a provider that emits the SAME tool call **twice** (two tool turns) then a final answer, with a prompter that returns `AllowSession` but counts how many times it was consulted; assert it was consulted **once**:
```rust
    struct CountingSessionPrompter { calls: std::sync::Arc<std::sync::atomic::AtomicUsize> }
    #[async_trait]
    impl Prompter for CountingSessionPrompter {
        async fn confirm(&mut self, _t: &str, _a: &str) -> entheai_permission::Grant {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            entheai_permission::Grant::AllowSession
        }
    }

    #[tokio::test]
    async fn allow_for_session_stops_reprompting() {
        // provider: echo tool call, echo tool call, final answer.
        let provider = RecordingProvider {
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(vec![
                tool_call("echo", "{}"), tool_call("echo", "{}"),
                final_answer("done"),
            ].into_iter().flatten().collect()),
        };
        let agent = Agent::new(provider, "m".into());
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let policy = Policy::new(false, vec![]); // non-yolo -> Ask
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut prompter = CountingSessionPrompter { calls: calls.clone() };
        let ans = agent.run_task(vec![ChatMessage::user("go")], &registry, &policy, &mut prompter, None).await.unwrap();
        assert_eq!(ans, "done");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "second echo must not re-prompt");
    }
```
(Add tiny helpers `fn tool_call(name,args)->Vec<AssistantResponse>` returning one tool-call response, and `fn final_answer(s)->Vec<AssistantResponse>` returning one final response — or reuse/adjust the existing `tool_call_then_final`.)
- [ ] **Step 2: Run — FAIL.** `cargo test -p entheai-core allow_for_session`
- [ ] **Step 3: Implement** — in `dispatch_call`, change the `Ask` branch:
```rust
        let allowed = match policy.decide(name) {
            Decision::Allow => true,
            Decision::Deny => false,
            Decision::Ask => match prompter.confirm(name, &call.function.arguments).await {
                Grant::Deny => false,
                Grant::Allow => true,
                Grant::AllowSession => {
                    policy.grant_session(name);
                    true
                }
            },
        };
```
Add `use entheai_permission::Grant;` (alongside the existing `Decision` import).
- [ ] **Step 4: Run — PASS** (+ existing core tests). `cargo nextest run -p entheai-core`
- [ ] **Step 5: Commit + push**
```bash
git add crates/core/src/lib.rs
git commit -m "feat(core): dispatch_call honors AllowSession — grow the session allowlist"
git push origin main
```

---

### Task 4: the `todo` tool + shared plan types

**Files:** Create `crates/tools/src/todo.rs`; Modify `crates/tools/src/lib.rs` (`pub mod todo;`)

- [ ] **Step 1: Failing test** — `crates/tools/src/todo.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_items_and_statuses() {
        let v = serde_json::json!({"items":[
            {"text":"read","status":"done"},
            {"text":"map","status":"in_progress"},
            {"text":"add","status":"pending"},
            {"text":"weird","status":"???"}
        ]});
        let items = parse_todos(&v);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].status, TodoStatus::Done);
        assert_eq!(items[1].status, TodoStatus::InProgress);
        assert_eq!(items[3].status, TodoStatus::Pending); // unknown -> pending
        assert_eq!(items[0].text, "read");
    }
    #[test]
    fn parse_bad_json_is_empty() {
        assert!(parse_todos(&serde_json::json!({"nope":1})).is_empty());
    }
    #[tokio::test]
    async fn todo_tool_confirms_count() {
        let out = TodoTool.call(serde_json::json!({"items":[{"text":"a","status":"pending"}]})).await.unwrap();
        assert!(out.contains('1'));
    }
}
```
Ensure `crates/tools/Cargo.toml` has `tokio` (`macros`,`rt`) dev-deps (mirror the fs tests).
- [ ] **Step 2: Run — FAIL.** `cargo test -p entheai-tools todo`
- [ ] **Step 3: Implement** — `crates/tools/src/todo.rs`:
```rust
use crate::{Tool, ToolError};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus { Pending, InProgress, Done, Failed }

#[derive(Debug, Clone)]
pub struct TodoItem { pub text: String, pub status: TodoStatus }

/// Parse a `{ "items": [ { "text", "status" } ] }` payload into plan items.
/// Unknown/missing status -> Pending; non-object/absent items -> empty.
pub fn parse_todos(args: &Value) -> Vec<TodoItem> {
    let Some(items) = args.get("items").and_then(|v| v.as_array()) else { return Vec::new() };
    items.iter().filter_map(|it| {
        let text = it.get("text")?.as_str()?.to_string();
        let status = match it.get("status").and_then(|s| s.as_str()).unwrap_or("pending") {
            "in_progress" => TodoStatus::InProgress,
            "done" => TodoStatus::Done,
            "failed" => TodoStatus::Failed,
            _ => TodoStatus::Pending,
        };
        Some(TodoItem { text, status })
    }).collect()
}

/// The `todo` tool: the model publishes/updates its plan. The TUI reads the same
/// payload off `ToolStarted` to render the plan pane; this just validates + acks.
pub struct TodoTool;

#[async_trait::async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str { "todo" }
    fn schema(&self) -> Value {
        serde_json::json!({
            "type":"function",
            "function":{
                "name":"todo",
                "description":"Publish or update your task plan. Call it with the FULL list each time; set each item's status to pending/in_progress/done/failed as you work.",
                "parameters":{"type":"object","properties":{"items":{"type":"array","items":{
                    "type":"object",
                    "properties":{"text":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","done","failed"]}},
                    "required":["text","status"]}}},"required":["items"]}
            }
        })
    }
    async fn call(&self, args: Value) -> Result<String, ToolError> {
        Ok(format!("plan: {} item(s)", parse_todos(&args).len()))
    }
}
```
Add `pub mod todo;` to `crates/tools/src/lib.rs`.
- [ ] **Step 4: Run — PASS.** `cargo test -p entheai-tools todo`
- [ ] **Step 5: Commit + push**
```bash
git add crates/tools/src/todo.rs crates/tools/src/lib.rs crates/tools/Cargo.toml
git commit -m "feat(tools): todo tool + shared TodoItem/parse_todos (plan pane source)"
git push origin main
```

---

### Task 5: fan-out decomposition carries plan labels

**Files:** Modify `crates/orchestrator/src/lib.rs`

- [ ] **Step 1: Failing test** — `crates/orchestrator/src/lib.rs` tests (pure): assert the variant shape:
```rust
    #[test]
    fn decomposed_carries_tasks() {
        let ev = FanoutEvent::Decomposed { tasks: vec![("coder".into(), "add x".into())] };
        if let FanoutEvent::Decomposed { tasks } = ev {
            assert_eq!(tasks[0].1, "add x");
        } else { panic!() }
    }
```
- [ ] **Step 2: Run — FAIL.** `cargo test -p entheai-orchestrator decomposed_carries`
- [ ] **Step 3: Implement** — change the variant `Decomposed { count: usize }` → `Decomposed { tasks: Vec<(String, String)> }` (role, task). In `run_fanout`, emit it from the parsed `subtasks`:
```rust
    if let Some(tx) = &events {
        let _ = tx.send(FanoutEvent::Decomposed {
            tasks: subtasks.iter().map(|s| (s.role.clone(), s.task.clone())).collect(),
        });
    }
```
- [ ] **Step 4: Run — PASS** (+ orchestrator tests). `cargo nextest run -p entheai-orchestrator`
- [ ] **Step 5: Commit + push**
```bash
git add crates/orchestrator/src/lib.rs
git commit -m "feat(orchestrator): FanoutEvent::Decomposed carries sub-task labels for the plan pane"
git push origin main
```

---

### Task 6: register + advertise the `todo` tool (bin)

**Files:** Modify `bin/entheai/src/main.rs`

- [ ] **Step 1: Change** — in `build_tools`, register `TodoTool` after the built-ins:
```rust
    registry.register(Box::new(entheai_tools::todo::TodoTool));
```
and fold a plan instruction into the system prompt. After computing `system_prompt`, if it's `None` set a base one, and always append the todo hint. Simplest: build a base string and append the skills advertisement:
```rust
    let todo_hint = "Use the `todo` tool to publish and keep your plan up to date — set items to in_progress/done as you work.";
    let system_prompt = Some(match system_prompt {
        Some(skills_ad) => format!("{skills_ad}\n\n{todo_hint}"),
        None => todo_hint.to_string(),
    });
```
- [ ] **Step 2: Verify** — `cargo build --release -p entheai` succeeds; `cargo run -q -p entheai -- --no-companion --model "nope/x" "hi"` still prints `Error: unknown provider 'nope'` (config still parses; todo registered before the provider error is irrelevant — the tool registration is in build_tools which runs before model resolution).
- [ ] **Step 3: Commit + push**
```bash
git add bin/entheai/src/main.rs
git commit -m "feat(bin): register + advertise the todo tool"
git push origin main
```

---

### Task 7: enriched progress line (verb + token tally)

**Files:** Modify `crates/tui/src/lib.rs`

- [ ] **Step 1: Failing tests** — add pure helpers + tests in `crates/tui/src/lib.rs`:
```rust
    #[test]
    fn fmt_tokens_scales() {
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(18_432), "18.4k");
        assert_eq!(fmt_tokens(1_250_000), "1.2M");
    }
    #[test]
    fn verb_rotates_deterministically() {
        assert_eq!(verb_for(0), VERBS[0]);
        assert_eq!(verb_for(VERBS.len()), VERBS[0]); // wraps
        assert_ne!(verb_for(0), verb_for(1));
    }
```
- [ ] **Step 2: Run — FAIL.** `cargo test -p entheai-tui fmt_tokens`
- [ ] **Step 3: Implement** — add:
```rust
const VERBS: [&str; 8] = ["Thinking","Churning","Weaving","Reasoning","Wrangling","Cooking","Threading","Brewing"];
fn verb_for(idx: usize) -> &'static str { VERBS[idx % VERBS.len()] }
fn fmt_tokens(n: usize) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{:.1}k", n as f64 / 1_000.0) }
    else { n.to_string() }
}
```
Add `out_tokens: usize` and `verb_idx: usize` to `App` (init 0). In the `AgentEvent::Token` handler: `app.out_tokens += t.len() / 4;` (and still append to the streaming bubble). On `Action::Submit`: `app.out_tokens = 0; app.verb_idx = app.verb_idx.wrapping_add(1);`. In `render`, the `Status::Working` progress line becomes (use `current_action` when it's a running-tool string, else the verb):
```rust
        Status::Working => {
            let elapsed = app.run_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            let label = if app.current_action.starts_with("running ") {
                app.current_action.clone()
            } else {
                format!("{}…", verb_for(app.verb_idx))
            };
            Line::from(vec![
                Span::styled(FRAMES[app.spinner_frame], Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(
                    format!("{label} · {elapsed}s · ↓{} tokens", fmt_tokens(app.out_tokens)),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ])
        }
```
- [ ] **Step 4: Run — PASS** (+ tui tests). `cargo nextest run -p entheai-tui`
- [ ] **Step 5: Commit + push**
```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): progress line — rotating verb + live output-token tally"
git push origin main
```

---

### Task 8: the plan pane

**Files:** Modify `crates/tui/src/lib.rs`

- [ ] **Step 1: Failing tests** — a pure `plan_lines(plan, width) -> Vec<Line>` renderer:
```rust
    #[test]
    fn plan_lines_markers_and_empty() {
        use entheai_tools::todo::{TodoItem, TodoStatus};
        assert!(plan_lines(&[], 40).is_empty()); // empty -> no rows
        let plan = vec![
            TodoItem{text:"read".into(), status:TodoStatus::Done},
            TodoItem{text:"map".into(), status:TodoStatus::InProgress},
            TodoItem{text:"add".into(), status:TodoStatus::Pending},
        ];
        let lines = plan_lines(&plan, 40);
        assert_eq!(lines.len(), 3);
        // render to strings to check markers
        let s: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect()).collect();
        assert!(s[0].starts_with("✓"));
        assert!(s[1].starts_with("◐"));
        assert!(s[2].starts_with("◻"));
    }
```
- [ ] **Step 2: Run — FAIL.** `cargo test -p entheai-tui plan_lines`
- [ ] **Step 3: Implement** —
```rust
fn plan_lines(plan: &[entheai_tools::todo::TodoItem], width: u16) -> Vec<Line<'static>> {
    use entheai_tools::todo::TodoStatus;
    let w = (width.max(4) as usize).saturating_sub(2);
    plan.iter().map(|it| {
        let (marker, style) = match it.status {
            TodoStatus::Pending => ("◻", Style::default().add_modifier(Modifier::DIM)),
            TodoStatus::InProgress => ("◐", Style::default().fg(Color::Cyan)),
            TodoStatus::Done => ("✓", Style::default().fg(Color::Green)),
            TodoStatus::Failed => ("✗", Style::default().fg(Color::Red)),
        };
        Line::styled(format!("{marker} {}", truncate(&it.text, w)), style)
    }).collect()
}
```
Add `plan: Vec<entheai_tools::todo::TodoItem>` to `App` (init `Vec::new()`). Wire updates:
- `AgentEvent::ToolStarted { name, args }` handler: when `name == "todo"`, `app.plan = entheai_tools::todo::parse_todos(&serde_json::from_str(&args).unwrap_or(serde_json::Value::Null));` (still push the `⚙ todo(...)` tool line as today, or skip it for `todo`).
- `FanoutEvent::Decomposed { tasks }` → seed `app.plan = tasks.iter().map(|(role, task)| TodoItem{ text: format!("[{role}] {task}"), status: Pending }).collect();`
- `FanoutEvent::CoderStarted { index, .. }` → set `app.plan[index].status = InProgress` (bounds-check).
- `FanoutEvent::CoderFinished { index, committed, status }` → `Done` if status indicates success else `Failed` (bounds-check).
- On submit + on run result → `app.plan.clear()`.
Add a layout region. Change `Layout::vertical` to insert a plan region whose height is `plan_h = min(app.plan.len()+? , cap)`; when `app.plan.is_empty()`, height 0. Compute `let plan_rows = if app.plan.is_empty() { 0 } else { (app.plan.len() as u16).min(8) };` and add a `Constraint::Length(plan_rows)` between history and progress; render `Paragraph::new(plan_lines(&app.plan, width)).block(Block::default().borders(Borders::ALL).title("plan"))` — but that adds 2 border rows; simpler v1: render the plan lines WITHOUT a box (dim, prefixed) using `Constraint::Length(plan_rows)` and a `Paragraph` with no block. (Box optional; keep v1 boxless to avoid border-row math.) Update `history_height` math to subtract `plan_rows`.
- [ ] **Step 4: Run — PASS** (+ tui tests). `cargo nextest run -p entheai-tui`
- [ ] **Step 5: Commit + push**
```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): live plan pane (todo tool + fan-out), collapses when empty"
git push origin main
```

---

### Task 9: 3-option permission modal

**Files:** Modify `crates/tui/src/lib.rs`

- [ ] **Step 1: Change** — `TuiPrompter` must return `Grant`. Change the oneshot channel type: `PermissionRequest { tool, args, respond: oneshot::Sender<entheai_permission::Grant> }`; `TuiPrompter::confirm` returns `Grant` (`rx.await.unwrap_or(Grant::Deny)`); `pending_permission: Option<oneshot::Sender<Grant>>`. In `handle_key` under `Status::AwaitingPermission`:
```rust
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => { if let Some(tx)=app.pending_permission.take() { let _=tx.send(entheai_permission::Grant::Allow); } app.status=Status::Working; }
            KeyCode::Char('a') | KeyCode::Char('A') => { if let Some(tx)=app.pending_permission.take() { let _=tx.send(entheai_permission::Grant::AllowSession); } app.status=Status::Working; }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => { if let Some(tx)=app.pending_permission.take() { let _=tx.send(entheai_permission::Grant::Deny); } app.status=Status::Working; }
            _ => {}
        }
```
Update the modal text in `render`: `format!("allow {tool}({args})?  [y]es · [n]o · [a]llow for session")`.
- [ ] **Step 2: Verify** — `cargo build -p entheai-tui` compiles; existing tui tests pass. (This is integration-shaped; the unit-testable pieces are covered — assert compilation + a headless smoke below.)
- [ ] **Step 3: Headless smoke** — `printf 'q' | cargo run -q -p entheai -- --no-companion 2>&1 | head -3 || true` (TTY error acceptable).
- [ ] **Step 4: Run** — `cargo nextest run -p entheai-tui` (all pass).
- [ ] **Step 5: Commit + push**
```bash
git add crates/tui/src/lib.rs
git commit -m "feat(tui): 3-option permission modal (y/n/allow-for-session)"
git push origin main
```

---

### Task 10: idle-frugal render + line cache

**Files:** Modify `crates/tui/src/lib.rs`

- [ ] **Step 1: Failing test** — a cache that rebuilds only on change. Add a `LineCache { key: Option<(usize, u16)>, lines: Vec<Line<'static>> }` with `get_or_build(messages, width) -> &[Line]` keyed by `(messages.len(), width)` **plus** the last message's text length (cheap change signal):
```rust
    #[test]
    fn line_cache_rebuilds_on_change_only() {
        let mut c = LineCache::default();
        let mut msgs = vec![Msg{role:Role::User, text:"hi".into()}];
        let a = c.get_or_build(&msgs, 40).len();
        let b = c.get_or_build(&msgs, 40).len();   // same key -> no rebuild
        assert_eq!(c.rebuilds, 1);
        assert_eq!(a, b);
        msgs.push(Msg{role:Role::Assistant, text:"yo".into()});
        c.get_or_build(&msgs, 40);                 // changed -> rebuild
        assert_eq!(c.rebuilds, 2);
    }
```
- [ ] **Step 2: Run — FAIL.** `cargo test -p entheai-tui line_cache`
- [ ] **Step 3: Implement** —
```rust
#[derive(Default)]
struct LineCache {
    key: Option<(usize, usize, u16)>, // (msg count, last-msg len, width)
    lines: Vec<Line<'static>>,
    rebuilds: usize,
}
impl LineCache {
    fn get_or_build(&mut self, messages: &[Msg], width: u16) -> &[Line<'static>] {
        let last_len = messages.last().map(|m| m.text.len()).unwrap_or(0);
        let key = (messages.len(), last_len, width);
        if self.key != Some(key) {
            self.lines = build_history_lines(messages, width);
            self.key = Some(key);
            self.rebuilds += 1;
        }
        &self.lines
    }
}
```
Use it in the loop: replace the per-iteration `build_history_lines(...)` with `line_cache.get_or_build(&app.messages, size.width)` (hold a `let mut line_cache = LineCache::default();` before the loop). Then make the loop **draw only when needed**: track `let mut dirty = true;`; set `dirty = true` on every arm that changes state (key/perm/result/progress/fanout), and on the ticker arm only while `Status::Working` (spinner animating). Draw at the top of the loop only `if dirty { terminal.draw(...)?; dirty = false; }`. (Streaming tokens set dirty; idle Idle status with no events → no redraw.)
- [ ] **Step 4: Run — PASS** (+ tui tests) + headless smoke still works. `cargo nextest run -p entheai-tui`
- [ ] **Step 5: Commit + push**
```bash
git add crates/tui/src/lib.rs
git commit -m "perf(tui): cache wrapped history lines + dirty-flag redraw (idle-frugal)"
git push origin main
```

---

## Self-Review

**Spec coverage:** §2 progress line → Task 7 (verb + token tally; effort omitted per spec). §3 plan pane → Task 8 (markers, empty→0 rows, truncation). §4 sources → Task 4 (`todo` tool + parse) + Task 5 (fan-out labels) + Task 8 (wiring both). §5 permission flow → Tasks 1 (`Grant`) + 2 (`Policy` session set) + 3 (`dispatch_call`) + 9 (3-option modal). §6 feel/perf → Task 10 (line cache + dirty-flag idle-frugal render); animation = the existing spinner tick (kept). §7 architecture → the crate split matches Tasks 1–10. §8 testing → each task carries its tests. **No gaps** (interactive animation polish beyond the spinner + a universal effort metric are explicit non-goals).

**Placeholder scan:** no `TODO`/"handle edge cases". Task 9 is integration-shaped (permission modal wiring) and says so, with unit-testable pieces covered in Tasks 1–3 and a headless smoke; Task 8's box-vs-boxless plan render picks boxless for v1 explicitly.

**Type consistency:** `Grant{Deny,Allow,AllowSession}`, `Policy::new(yolo, allowlist)` + `grant_session`/`decide`, `TodoStatus{Pending,InProgress,Done,Failed}` + `TodoItem{text,status}` + `parse_todos`, `FanoutEvent::Decomposed{tasks: Vec<(String,String)>}`, `verb_for`/`fmt_tokens`/`plan_lines`/`LineCache` are used consistently across Tasks 1–10.
