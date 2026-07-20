//! Interactive `ratatui` chat UI driving the agentic `run_task` loop.
//!
//! Flow: the user types a message and presses Enter; a tokio task runs
//! `run_task` and streams the outcome back over an mpsc channel. When the model
//! wants to call a gated tool, [`TuiPrompter`] forwards a permission request to
//! the UI thread, which pops a modal and answers via a oneshot channel.
//!
//! v1 scope: type -> run -> permission modal -> answer/error in history. No
//! token streaming, no inline tool-progress, no diffs, no sessions.

use std::io::Stdout;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame, Terminal,
};
use tokio::sync::{mpsc, oneshot};

use entheai_companion::state::StateChange;
use entheai_core::{Agent, AgentEvent};
use entheai_permission::{Grant, Policy, Prompter};
use entheai_providers::{ChatMessage, Provider};
use entheai_radio::{Command as RadioCommand, Event as RadioEvent, Radio};
use entheai_tools::ToolRegistry;

/// Spinner animation frames for the live progress line (Charm/Bubbletea-style
/// braille spinner), advanced on each animation tick while a run is in flight.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Rotating verbs for the progress line when no tool is currently running, one
/// picked per submitted turn (see `App.verb_idx`) so repeated runs don't always
/// say "Thinking".
const VERBS: [&str; 8] = [
    "Thinking",
    "Churning",
    "Weaving",
    "Reasoning",
    "Wrangling",
    "Cooking",
    "Threading",
    "Brewing",
];

/// The verb for turn index `idx`, wrapping around `VERBS`.
fn verb_for(idx: usize) -> &'static str {
    VERBS[idx % VERBS.len()]
}

/// Format a token count for the live progress line: `950`, `18.4k`, `1.2M`.
fn fmt_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Render the live plan (from the `todo` tool or fan-out decomposition) as one
/// styled line per item: a status marker + truncated text. Empty plan -> no
/// rows (the caller collapses the pane's layout region to height 0).
fn plan_lines(plan: &[entheai_tools::todo::TodoItem], width: u16) -> Vec<Line<'static>> {
    use entheai_tools::todo::TodoStatus;
    let w = (width.max(4) as usize).saturating_sub(2);
    plan.iter()
        .map(|it| {
            let (marker, style) = match it.status {
                TodoStatus::Pending => ("◻", Style::default().add_modifier(Modifier::DIM)),
                TodoStatus::InProgress => ("◐", Style::default().fg(Color::Cyan)),
                TodoStatus::Done => ("✓", Style::default().fg(Color::Green)),
                TodoStatus::Failed => ("✗", Style::default().fg(Color::Red)),
            };
            Line::styled(format!("{marker} {}", truncate(&it.text, w)), style)
        })
        .collect()
}

type Backend = CrosstermBackend<Stdout>;

/// Who authored a line of history.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    /// Inline tool-call/result lines pushed as `ToolStarted`/`ToolFinished`
    /// events arrive; display-only (never fed back into `build_history`).
    Tool,
    Error,
}

impl Role {
    /// The line prefix, style, and whether the row background is filled to the
    /// full width (so the role reads as a distinct block).
    fn style(self) -> (&'static str, Style, bool) {
        match self {
            Role::User => (
                "you> ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                false,
            ),
            Role::Assistant => (
                "entheai> ",
                Style::default()
                    .fg(Color::Rgb(224, 226, 240))
                    .bg(Color::Rgb(32, 34, 46)),
                true,
            ),
            Role::Tool => (
                "tool> ",
                Style::default().add_modifier(Modifier::DIM),
                false,
            ),
            Role::Error => (
                "error> ",
                Style::default()
                    .fg(Color::Rgb(255, 130, 130))
                    .bg(Color::Rgb(48, 26, 30)),
                true,
            ),
        }
    }
}

/// One rendered entry in the scrollback.
struct Msg {
    role: Role,
    text: String,
}

/// What the UI is currently doing.
enum Status {
    Idle,
    Working,
    AwaitingPermission { tool: String, args: String },
}

/// A tool-permission question raised by a running task, forwarded to the UI.
struct PermissionRequest {
    tool: String,
    args: String,
    respond: oneshot::Sender<Grant>,
}

/// `Prompter` impl used inside spawned run tasks: forwards each `confirm` to the
/// UI thread and awaits the user's grant over a oneshot.
struct TuiPrompter {
    tx: mpsc::Sender<PermissionRequest>,
}

#[async_trait]
impl Prompter for TuiPrompter {
    async fn confirm(&mut self, tool_name: &str, args_summary: &str) -> Grant {
        let (respond, rx) = oneshot::channel();
        let req = PermissionRequest {
            tool: tool_name.to_string(),
            args: args_summary.to_string(),
            respond,
        };
        if self.tx.send(req).await.is_err() {
            return Grant::Deny; // UI gone -> deny
        }
        rx.await.unwrap_or(Grant::Deny) // UI dropped the responder -> deny
    }
}

/// All mutable UI state.
struct App {
    messages: Vec<Msg>,
    input: String,
    status: Status,
    /// Vertical scroll offset into the wrapped history, in rows.
    scroll: u16,
    /// When true, the view sticks to the bottom as new content arrives.
    follow: bool,
    model_label: String,
    /// The responder for the modal currently on screen, if any.
    pending_permission: Option<oneshot::Sender<Grant>>,
    /// When the current run started; `None` while idle. Drives the elapsed-time
    /// display in the live progress line.
    run_started: Option<Instant>,
    /// Current frame index into [`FRAMES`] for the progress-line spinner.
    spinner_frame: usize,
    /// Human-readable description of what the agent is doing right now, e.g.
    /// "thinking" or "running read_file".
    current_action: String,
    /// Title of the radio track currently playing, shown in the status bar.
    now_playing: Option<String>,
    /// Whether this session runs submitted messages through fan-out
    /// (decompose → parallel coders → integrate) instead of the single-agent
    /// `run_task` loop. Set once at construction; shown in the status bar.
    fanout: bool,
    /// The `WorkerPool` backing the in-flight fan-out run, if any — set right
    /// before spawning `run_fanout`, cleared when that run finishes (same
    /// lifecycle as `fanout_rx`). `/workers list/stop/debug` read/mutate this.
    worker_pool: Option<Arc<entheai_orchestrator::WorkerPool>>,
    /// Optional system prompt (e.g. skills advertisement) prepended to the
    /// conversation history sent on each single-agent run.
    system_prompt: Option<String>,
    /// Index into `messages` of the assistant bubble currently being streamed
    /// into by live `AgentEvent::Token`s, if any.
    streaming_idx: Option<usize>,
    /// Running tally of output tokens for the current turn (reset on submit),
    /// estimated from streamed token text. Drives the `↓N tokens` progress
    /// readout.
    out_tokens: usize,
    /// Index into `VERBS`, advanced once per submitted turn so the progress
    /// line's idle verb varies run to run.
    verb_idx: usize,
    /// The live task plan, sourced from the `todo` tool (single-agent runs) or
    /// seeded/updated by fan-out lifecycle events. Empty -> the plan pane
    /// collapses to zero height.
    plan: Vec<entheai_tools::todo::TodoItem>,
    /// Live fan-out swarm model (fed from the same FanoutEvent stream as `plan`).
    swarm: entheai_viz::SwarmModel,
    /// Which main view is showing (chat vs full-screen swarm).
    view: ViewMode,
    /// Whether the swarm viz is enabled (from `[viz] swarm`).
    viz_swarm: bool,
    /// Timestamps of the last bare Esc / Ctrl-C, for double-tap detection:
    /// Esc-Esc stops the current run, Ctrl-C twice quits. Any intervening key
    /// clears them, so only a genuine double-tap fires.
    last_esc: Option<Instant>,
    last_ctrl_c: Option<Instant>,
    /// Transient one-line hint on the progress row (e.g. "press Esc again to
    /// stop the run"), cleared on the next key or when the window lapses.
    notice: Option<String>,
}

/// Which main view the TUI is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Chat,
    Swarm,
}

/// What a key press asked the loop to do.
enum Action {
    None,
    Quit,
    Submit(String),
    /// Ctrl-P: toggle radio pause/resume.
    RadioToggle,
    /// Ctrl-N: skip to the next radio track.
    RadioNext,
    /// Ctrl-V: toggle between the chat and full-screen swarm views.
    ViewToggle,
    /// Esc pressed twice while a run is in flight: abort it and return to idle.
    CancelRun,
}

// TODO(@rahulmranga): memory-v1 Task 9 — thread the shared memory into the
// interactive run loop. Add `memory: Option<std::sync::Arc<entheai_memory::MemoryRuntime>>`
// and `scope: entheai_memory::MemoryScope` params to `run` (and to `event_loop`),
// add `entheai-memory = { path = "../memory" }` to crates/tui/Cargo.toml, then swap
// the internal `.run_task(...)` call for `.run_task_with_memory(..., mem.as_deref(), sc)`
// with a per-turn `task_id`. Verbatim recipe: docs/superpowers/plans/2026-07-19-entheai-memory-v1.md → "Task 9".
/// Run the interactive TUI. Sets up the terminal, runs the event loop, and
/// always restores the terminal on exit (raw mode + alternate screen), even on
/// error, via [`TerminalGuard`].
#[allow(clippy::too_many_arguments)]
pub async fn run<P: Provider + 'static>(
    agent: Agent<P>,
    registry: ToolRegistry,
    policy: Policy,
    model_label: String,
    config: entheai_config::Config,
    root: std::path::PathBuf,
    fanout: bool,
    system_prompt: Option<String>,
    companion_tx: Option<tokio::sync::mpsc::UnboundedSender<StateChange>>,
) -> anyhow::Result<()> {
    let mut terminal = init_terminal()?;
    let guard = TerminalGuard;
    let result = event_loop(
        &mut terminal,
        agent,
        registry,
        policy,
        model_label,
        config,
        root,
        fanout,
        system_prompt,
        companion_tx,
    )
    .await;
    drop(guard); // restore the terminal before surfacing any error
    result
}

/// Restores the terminal on drop (covers early returns, `?`, and panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

fn init_terminal() -> anyhow::Result<Terminal<Backend>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

// Mirrors `run`'s signature 1:1 (terminal is the only addition) so the two
// stay easy to read side by side; grouping the fan-out/session params into a
// struct would obscure that correspondence for one extra argument.
#[allow(clippy::too_many_arguments)]
async fn event_loop<P: Provider + 'static>(
    terminal: &mut Terminal<Backend>,
    agent: Agent<P>,
    registry: ToolRegistry,
    policy: Policy,
    model_label: String,
    config: entheai_config::Config,
    root: std::path::PathBuf,
    fanout: bool,
    system_prompt: Option<String>,
    companion_tx: Option<tokio::sync::mpsc::UnboundedSender<StateChange>>,
) -> anyhow::Result<()> {
    // Federation event bus (F1): connect once per TUI session, fail-safe. Cloned
    // into each fan-out submit's tee. `None` when `[nats]` is off/unreachable →
    // the tee is a pure identity and the UI event flow is unchanged. Read
    // `config.nats` here, before `config` is moved into the `Arc` below.
    let bus = entheai_bus::Bus::connect(&entheai_bus::BusOptions::from_config(&config.nats)).await;

    // Arc so each spawned run task can share the agent/registry/policy/config.
    let agent = Arc::new(agent);
    let registry = Arc::new(registry);
    let policy = Arc::new(policy);
    let config = Arc::new(config);

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionRequest>(8);
    let (result_tx, mut result_rx) = mpsc::channel::<Result<String, String>>(8);
    // Receiver for the currently running task's progress events, if any. Set on
    // submit, torn down when the run's sender is dropped (channel closes) or the
    // result arrives.
    let mut events_rx: Option<mpsc::UnboundedReceiver<AgentEvent>> = None;
    // Receiver for the currently running fan-out's lifecycle events, if any.
    // Same lifecycle as `events_rx`, but only ever set in fan-out mode.
    let mut fanout_rx: Option<mpsc::UnboundedReceiver<entheai_orchestrator::FanoutEvent>> = None;
    // Handle to the in-flight run task (single-agent or fan-out) so double-Esc
    // can abort it; `None` while idle, cleared when a run completes normally.
    let mut run_handle: Option<tokio::task::JoinHandle<()>> = None;

    let mut app = App {
        messages: Vec::new(),
        input: String::new(),
        status: Status::Idle,
        scroll: 0,
        follow: true,
        model_label,
        pending_permission: None,
        run_started: None,
        spinner_frame: 0,
        current_action: "thinking".to_string(),
        now_playing: None,
        fanout,
        worker_pool: None,
        system_prompt,
        streaming_idx: None,
        out_tokens: 0,
        verb_idx: 0,
        plan: Vec::new(),
        swarm: entheai_viz::SwarmModel::new(),
        view: ViewMode::Chat,
        viz_swarm: config.viz.swarm,
        last_esc: None,
        last_ctrl_c: None,
        notice: None,
    };

    // Background music player (yt-dlp + rodio); one per TUI session.
    let mut radio = Radio::spawn(
        Radio::default_cache_dir(),
        config.radio.download_timeout_secs,
    )
    .expect("spawn radio thread");

    let mut events = EventStream::new();
    // Floor the tick at 16ms (~60fps) so a `tick_ms = 0` config can't spin a
    // 0ms busy-loop.
    let tick_ms = config.viz.tick_ms.max(16);
    let mut ticker = tokio::time::interval(Duration::from_millis(tick_ms));
    let mut line_cache = LineCache::default();
    // Env banner (folders · seeded machine id · ip) — computed once, shown on the
    // status bar's second row every frame.
    let env_line = env_status_line(&root);
    // Redraw gate: only `terminal.draw` when something visible changed, so an
    // idle session (no keys, no running task) doesn't repaint every tick.
    let mut dirty = true;

    loop {
        if dirty {
            // Clamp scroll against the current terminal size before drawing.
            let size = terminal.size()?;
            let plan_rows = plan_rows_for(app.plan.len(), config.viz.plan_rows_cap);
            let swarm_rows = if app.view == ViewMode::Chat {
                swarm_rows_for(app.viz_swarm, &app.swarm, config.viz.swarm_rows_cap)
            } else {
                0
            };
            let history_height = size
                .height
                .saturating_sub(STATUS_ROWS + PROGRESS_ROWS + INPUT_ROWS + plan_rows + swarm_rows);
            let lines = line_cache.get_or_build(&app.messages, size.width);
            let max_scroll = (lines.len() as u16).saturating_sub(history_height);
            if app.follow {
                app.scroll = max_scroll;
            } else {
                app.scroll = app.scroll.min(max_scroll);
                if app.scroll == max_scroll {
                    app.follow = true; // scrolled back to the bottom -> resume following
                }
            }
            let scroll = app.scroll;
            terminal.draw(|frame| {
                render(frame, &app, lines, scroll, plan_rows, swarm_rows, &env_line)
            })?;
            dirty = false;
        }

        tokio::select! {
            maybe_event = events.next() => {
                let Some(ev) = maybe_event else { break };
                let ev = match ev {
                    Ok(ev) => ev,
                    Err(_) => break,
                };
                match ev {
                    Event::Resize(_, _) => dirty = true,
                    Event::Key(key) => {
                    dirty = true;
                    match handle_key(&mut app, key) {
                        Action::Quit => break,
                        Action::None => {}
                        Action::RadioToggle => radio.send(RadioCommand::TogglePause),
                        Action::RadioNext => radio.send(RadioCommand::Next),
                        Action::ViewToggle => {
                            app.view = if app.view == ViewMode::Chat {
                                ViewMode::Swarm
                            } else {
                                ViewMode::Chat
                            };
                        }
                        Action::CancelRun => {
                            // Abort the in-flight run task; its result_tx is
                            // dropped, so we reset run state here rather than
                            // waiting on the (never-arriving) result.
                            if let Some(h) = run_handle.take() {
                                h.abort();
                            }
                            app.messages.push(Msg {
                                role: Role::Error,
                                text: "⛔ run stopped".to_string(),
                            });
                            app.status = Status::Idle;
                            app.follow = true;
                            app.run_started = None;
                            events_rx = None;
                            fanout_rx = None;
                            app.worker_pool = None;
                            app.streaming_idx = None;
                            app.plan.clear();
                            if let Some(ref tx) = companion_tx {
                                let _ = tx.send(StateChange::idle());
                            }
                        }
                        Action::Submit(text) if is_radio_command(&text) => {
                            handle_radio_command(&mut app, &radio, &text);
                        }
                        Action::Submit(text) if is_workers_command(&text) => {
                            handle_workers_command(&mut app, &text);
                        }
                        Action::Submit(text) if is_viz_command(&text) => {
                            handle_viz_command(&mut app, &text);
                        }
                        Action::Submit(text) if is_help_command(&text) => {
                            handle_help_command(&mut app);
                        }
                        Action::Submit(text) if is_clear_command(&text) => {
                            handle_clear_command(&mut app);
                        }
                        Action::Submit(text) if is_fanout_command(&text) => {
                            handle_fanout_command(&mut app, &text);
                        }
                        Action::Submit(text) if is_model_command(&text) => {
                            handle_model_command(&mut app);
                        }
                        Action::Submit(text) if is_quit_command(&text) => break,
                        Action::Submit(text) => {
                            app.messages.push(Msg { role: Role::User, text: text.clone() });
                            app.status = Status::Working;
                            if let Some(ref tx) = companion_tx {
                                let _ = tx.send(StateChange::working());
                            }
                            app.follow = true;
                            app.current_action = "thinking".to_string();
                            app.run_started = Some(Instant::now());
                            app.out_tokens = 0;
                            app.verb_idx = app.verb_idx.wrapping_add(1);
                            app.plan.clear();

                            if app.fanout {
                                let pool = entheai_orchestrator::WorkerPool::new(
                                    config.router.max_parallel.max(1),
                                );
                                app.worker_pool = Some(Arc::clone(&pool));
                                let config = Arc::clone(&config);
                                let root = root.clone();
                                let result_tx = result_tx.clone();
                                let (ftx, frx) =
                                    mpsc::unbounded_channel::<entheai_orchestrator::FanoutEvent>();
                                fanout_rx = Some(frx);
                                // Tee the UI event stream to the NATS bus (F1).
                                // Fresh per-run session id scopes the subjects;
                                // with the bus off this returns `Some(ftx)`
                                // unchanged. The BusSession is moved into the
                                // spawned task so it lives exactly as long as the
                                // run, then its Drop aborts the tee.
                                let (events, bus_session) = entheai_bus::tee(
                                    bus.clone(),
                                    entheai_bus::new_session_id(),
                                    Some(ftx),
                                );
                                run_handle = Some(tokio::spawn(async move {
                                    let res =
                                        entheai_orchestrator::run_fanout(&config, &root, &text, events, pool, None)
                                            .await;
                                    // Drain + flush the tee before dropping it so
                                    // the final events (e.g. `done`) reach
                                    // subscribers. No-op when NATS is off.
                                    bus_session.finish().await;
                                    let _ = result_tx.send(res.map_err(|e| e.to_string())).await;
                                }));
                            } else {
                                let history = build_history(app.system_prompt.as_deref(), &app.messages);

                                let agent = Arc::clone(&agent);
                                let registry = Arc::clone(&registry);
                                let policy = Arc::clone(&policy);
                                let perm_tx = perm_tx.clone();
                                let result_tx = result_tx.clone();
                                let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();
                                events_rx = Some(event_rx);
                                run_handle = Some(tokio::spawn(async move {
                                    let mut prompter = TuiPrompter { tx: perm_tx };
                                    let res = agent
                                        .run_task(history, &registry, &policy, &mut prompter, Some(event_tx))
                                        .await;
                                    let _ = result_tx.send(res.map_err(|e| e.to_string())).await;
                                }));
                            }
                        }
                    }
                    }
                    _ => {}
                }
            }
            Some(req) = perm_rx.recv() => {
                dirty = true;
                app.view = ViewMode::Chat; // ensure the y/n/a modal (chat-layout only) is visible
                app.pending_permission = Some(req.respond);
                if let Some(ref tx) = companion_tx {
                    let _ = tx.send(StateChange::permission_pending(&req.tool, &req.args));
                }
                app.status = Status::AwaitingPermission { tool: req.tool, args: req.args };
            }
            Some(result) = result_rx.recv() => {
                dirty = true;
                match result {
                    Ok(answer) => {
                        if let Some(idx) = app.streaming_idx {
                            // Authoritative final text overwrites whatever streamed in live.
                            app.messages[idx].text = answer;
                        } else {
                            // No tokens streamed this run (e.g. a tool-only path) -> push fresh.
                            app.messages.push(Msg { role: Role::Assistant, text: answer });
                        }
                    }
                    Err(err) => app.messages.push(Msg { role: Role::Error, text: err }),
                }
                app.status = Status::Idle;
                if let Some(ref tx) = companion_tx {
                    let _ = tx.send(StateChange::idle());
                }
                app.follow = true;
                app.run_started = None;
                events_rx = None;
                fanout_rx = None;
                run_handle = None;
                app.worker_pool = None;
                app.streaming_idx = None;
                app.plan.clear();
            }
            maybe_progress = async {
                match events_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                dirty = true;
                match maybe_progress {
                    Some(AgentEvent::Thinking) => {
                        app.current_action = "thinking".to_string();
                        // Finalize any reasoning bubble from a prior turn so the next
                        // turn's tokens start a fresh one.
                        app.streaming_idx = None;
                    }
                    Some(AgentEvent::Token(t)) => {
                        let idx = match app.streaming_idx {
                            Some(idx) => idx,
                            None => {
                                app.messages.push(Msg {
                                    role: Role::Assistant,
                                    text: String::new(),
                                });
                                let idx = app.messages.len() - 1;
                                app.streaming_idx = Some(idx);
                                idx
                            }
                        };
                        app.out_tokens += t.len() / 4;
                        app.messages[idx].text.push_str(&t);
                    }
                    Some(AgentEvent::ToolStarted { name, args }) => {
                        if name == "todo" {
                            let parsed: serde_json::Value =
                                serde_json::from_str(&args).unwrap_or(serde_json::Value::Null);
                            app.plan = entheai_tools::todo::parse_todos(&parsed);
                        }
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("⚙ {name}({})", truncate_args(&args, 80)),
                        });
                        app.current_action = format!("running {name}");
                        // Post-tool tokens start a new bubble.
                        app.streaming_idx = None;
                    }
                    Some(AgentEvent::ToolFinished { name: _, result }) => {
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("  ↳ {}", first_line_trunc(&result, 120)),
                        });
                        app.current_action = "thinking".to_string();
                    }
                    None => events_rx = None, // sender dropped -> run finished
                }
            }
            maybe_fanout = async {
                match fanout_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                dirty = true;
                match maybe_fanout {
                    Some(entheai_orchestrator::FanoutEvent::Fallback) => {
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: "⋔ not a git repo — read-only fan-out".to_string(),
                        });
                    }
                    Some(entheai_orchestrator::FanoutEvent::Decomposed { tasks }) => {
                        app.swarm.decompose(&tasks);
                        let count = tasks.len();
                        app.plan = tasks
                            .iter()
                            .map(|(role, task)| entheai_tools::todo::TodoItem {
                                text: format!("[{role}] {task}"),
                                status: entheai_tools::todo::TodoStatus::Pending,
                            })
                            .collect();
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("◇ decomposed into {count} sub-task(s)"),
                        });
                        app.current_action = "fanning out".to_string();
                    }
                    Some(entheai_orchestrator::FanoutEvent::CoderStarted { index, role, task }) => {
                        app.swarm.coder_started(index, &role, &task);
                        if let Some(item) = app.plan.get_mut(index) {
                            item.status = entheai_tools::todo::TodoStatus::InProgress;
                        }
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("▸ [{role} #{index}] {}", truncate(&task, 80)),
                        });
                        app.current_action = "running coders".to_string();
                    }
                    Some(entheai_orchestrator::FanoutEvent::CoderFinished { index, committed, status }) => {
                        app.swarm.coder_finished(index, committed, &status);
                        if let Some(item) = app.plan.get_mut(index) {
                            item.status = if status.contains("fail") {
                                entheai_tools::todo::TodoStatus::Failed
                            } else {
                                entheai_tools::todo::TodoStatus::Done
                            };
                        }
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("  #{index}: {status}"),
                        });
                    }
                    Some(entheai_orchestrator::FanoutEvent::Integrating { branches }) => {
                        app.swarm.integrating(branches);
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("⧉ integrating {branches} branch(es)…"),
                        });
                        app.current_action = "integrating".to_string();
                    }
                    Some(entheai_orchestrator::FanoutEvent::Done { integration_branch, merged, conflicted }) => {
                        app.swarm.done(integration_branch.clone(), merged, conflicted);
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!(
                                "◆ done — {merged} merged, {conflicted} conflicted{}",
                                integration_branch.map(|b| format!(" · branch {b}")).unwrap_or_default()
                            ),
                        });
                    }
                    None => {
                        fanout_rx = None; // sender dropped -> run finished
                        app.worker_pool = None;
                    }
                }
            }
            Some(rev) = radio.next_event() => {
                dirty = true;
                handle_radio_event(&mut app, rev);
            }
            _ = ticker.tick() => {
                // Only the spinner animates while idle-frugal; a run in flight
                // needs a redraw each tick to advance it, otherwise the ticker
                // is a no-op frame (no dirty flag).
                if matches!(app.status, Status::Working) {
                    app.spinner_frame = (app.spinner_frame + 1) % FRAMES.len();
                    dirty = true;
                }
                // Auto-dismiss a double-tap hint once its window has lapsed.
                if app.notice.is_some()
                    && app.last_esc.map_or(true, |t| t.elapsed() >= DOUBLE_TAP)
                    && app.last_ctrl_c.map_or(true, |t| t.elapsed() >= DOUBLE_TAP)
                {
                    app.notice = None;
                    dirty = true;
                }
            }
        }
    }

    Ok(())
}

const STATUS_ROWS: u16 = 2; // row 1: entheai · model · state (+ ctx/tokens); row 2: env banner
const PROGRESS_ROWS: u16 = 1;
const INPUT_ROWS: u16 = 3;
/// Window within which a repeated Esc / Ctrl-C counts as a deliberate
/// double-tap (Esc-Esc stops a run; Ctrl-C twice quits).
const DOUBLE_TAP: Duration = Duration::from_millis(1200);

/// Height of the plan-pane layout region for a plan with `plan_len` items:
/// zero when empty (the pane collapses), capped at `cap` rows (from
/// `[viz].plan_rows_cap`) so a long plan can't crowd out the history entirely.
fn plan_rows_for(plan_len: usize, cap: u16) -> u16 {
    if plan_len == 0 {
        0
    } else {
        (plan_len as u16).min(cap)
    }
}

/// Inline swarm-pane height: 0 unless enabled AND a fan-out is active; otherwise
/// `min(nodes + 2 border, cap)` where `cap` comes from `[viz].swarm_rows_cap`.
/// Zero → the pane collapses.
fn swarm_rows_for(enabled: bool, model: &entheai_viz::SwarmModel, cap: u16) -> u16 {
    if !enabled || !model.is_active() || model.nodes.is_empty() {
        0
    } else {
        ((model.nodes.len() as u16) + 2).min(cap)
    }
}

/// Map a key press to an [`Action`], mutating input/scroll/modal state as needed.
fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    if key.kind != KeyEventKind::Press {
        return Action::None;
    }

    // The permission modal captures all keys while it is up.
    if matches!(app.status, Status::AwaitingPermission { .. }) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(tx) = app.pending_permission.take() {
                    let _ = tx.send(Grant::Allow);
                }
                app.status = Status::Working;
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if let Some(tx) = app.pending_permission.take() {
                    let _ = tx.send(Grant::AllowSession);
                }
                app.status = Status::Working;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                if let Some(tx) = app.pending_permission.take() {
                    let _ = tx.send(Grant::Deny);
                }
                app.status = Status::Working;
            }
            _ => {}
        }
        return Action::None;
    }

    let idle = matches!(app.status, Status::Idle);

    // Double-tap bookkeeping: take the prior Esc / Ctrl-C timestamps so any
    // intervening key breaks the chain; the two arms below re-arm their own.
    let prev_esc = app.last_esc.take();
    let prev_ctrl_c = app.last_ctrl_c.take();
    // Any key dismisses a transient hint; the arms below re-set it as needed.
    app.notice = None;

    match key.code {
        // Radio transport keys work whether idle or mid-run.
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Action::RadioToggle;
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Action::RadioNext;
        }
        // Ctrl-V toggles the full-screen swarm view, idle or mid-run.
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Action::ViewToggle;
        }
        // Ctrl-C twice quits (any state): the first press arms + hints, a
        // second within the double-tap window exits.
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let now = Instant::now();
            if prev_ctrl_c.is_some_and(|t| now.duration_since(t) < DOUBLE_TAP) {
                return Action::Quit;
            }
            app.last_ctrl_c = Some(now);
            app.notice = Some("press Ctrl-C again to quit".to_string());
        }
        // Esc twice stops the current run; when idle it clears the input line.
        // (A single Esc no longer quits — quitting is Ctrl-C ×2 or bare `q`.)
        KeyCode::Esc => {
            let now = Instant::now();
            if prev_esc.is_some_and(|t| now.duration_since(t) < DOUBLE_TAP) {
                if matches!(app.status, Status::Working) {
                    return Action::CancelRun;
                }
                app.input.clear();
            } else {
                app.last_esc = Some(now);
                app.notice = Some(
                    if matches!(app.status, Status::Working) {
                        "press Esc again to stop the run"
                    } else {
                        "press Esc again to clear the input"
                    }
                    .to_string(),
                );
            }
        }
        // Bare `q` quits only when idle AND the input is empty, so a chat message
        // can still contain the letter q.
        KeyCode::Char('q') if idle && app.input.is_empty() => return Action::Quit,
        KeyCode::Enter => {
            let trimmed = app.input.trim();
            // Commands safe to run mid-flight (read-only or run-management). The
            // state-mutating ones (/clear, /fanout, /quit) stay idle-only.
            let is_local_command = is_radio_command(trimmed)
                || is_workers_command(trimmed)
                || is_viz_command(trimmed)
                || is_help_command(trimmed)
                || is_model_command(trimmed);
            if !trimmed.is_empty() && (idle || is_local_command) {
                return Action::Submit(std::mem::take(&mut app.input));
            }
        }
        // Tab completes the slash-command name to the first match (while the
        // command name is still being typed — not once args have started).
        KeyCode::Tab if app.input.starts_with('/') && !app.input.trim().contains(' ') => {
            if let Some((cmd, _)) = slash_matches(&app.input).first() {
                app.input = format!("{cmd} ");
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(ch) => app.input.push(ch),
        KeyCode::PageUp => {
            app.follow = false;
            app.scroll = app.scroll.saturating_sub(5);
        }
        KeyCode::Up => {
            app.follow = false;
            app.scroll = app.scroll.saturating_sub(1);
        }
        KeyCode::PageDown => app.scroll = app.scroll.saturating_add(5),
        KeyCode::Down => app.scroll = app.scroll.saturating_add(1),
        _ => {}
    }

    Action::None
}

/// True when the submitted input is a local `/radio` command (never sent to the
/// agent).
fn is_radio_command(text: &str) -> bool {
    let t = text.trim_start();
    t == "/radio" || t.starts_with("/radio ")
}

/// Parse and dispatch a `/radio` command, echoing feedback into the history.
///
/// Forms: `/radio <url>` · `/radio add <url>` · `/radio pause` · `/radio next`
/// · `/radio stop` · `/radio` (help).
fn handle_radio_command(app: &mut App, radio: &Radio, text: &str) {
    app.messages.push(Msg {
        role: Role::User,
        text: text.to_string(),
    });
    let mut parts = text.split_whitespace().skip(1); // skip "/radio"
    let feedback = match (parts.next(), parts.next()) {
        (Some("add"), Some(url)) => {
            radio.send(RadioCommand::Add(url.to_string()));
            format!("♪ fetching {url}")
        }
        (Some(url), None) if url.starts_with("http") => {
            radio.send(RadioCommand::Add(url.to_string()));
            format!("♪ fetching {url}")
        }
        (Some("pause"), None) | (Some("resume"), None) => {
            radio.send(RadioCommand::TogglePause);
            "♪ toggled pause (Ctrl-P)".to_string()
        }
        (Some("next"), None) | (Some("skip"), None) => {
            radio.send(RadioCommand::Next);
            "♪ skipping (Ctrl-N)".to_string()
        }
        (Some("stop"), None) => {
            radio.send(RadioCommand::Stop);
            "♪ stopping".to_string()
        }
        _ => "usage: /radio <url> | add <url> | pause | next | stop  (Ctrl-P pause, Ctrl-N next)"
            .to_string(),
    };
    app.messages.push(Msg {
        role: Role::Tool,
        text: feedback,
    });
    app.follow = true;
}

/// True when the submitted input is a local `/workers` command (never sent to
/// the agent).
fn is_workers_command(text: &str) -> bool {
    let t = text.trim_start();
    t == "/workers" || t.starts_with("/workers ")
}

/// Human-readable rendering of a worker's status for `/workers list`/`debug`.
fn format_status(status: &entheai_orchestrator::WorkerStatus) -> String {
    use entheai_orchestrator::WorkerStatus;
    match status {
        WorkerStatus::Queued => "queued".to_string(),
        WorkerStatus::Running { started_at } => {
            format!("running {}s", started_at.elapsed().as_secs())
        }
        WorkerStatus::Done => "done".to_string(),
        WorkerStatus::TimedOut => "timed out".to_string(),
        WorkerStatus::Killed => "killed".to_string(),
    }
}

/// Parse and dispatch a `/workers [list, stop <id>, debug <id>]` command
/// against the in-flight fan-out's `WorkerPool` (if any), echoing feedback
/// into the history.
///
/// Forms: `/workers` / `/workers list` · `/workers stop <id>` · `/workers debug <id>`.
fn handle_workers_command(app: &mut App, text: &str) {
    app.messages.push(Msg {
        role: Role::User,
        text: text.to_string(),
    });
    let mut parts = text.split_whitespace().skip(1); // skip "/workers"
    let feedback = match &app.worker_pool {
        None => "no fan-out running".to_string(),
        Some(pool) => match (parts.next(), parts.next()) {
            (None, None) | (Some("list"), None) => {
                let mut summaries = pool.list();
                if summaries.is_empty() {
                    "no workers".to_string()
                } else {
                    summaries.sort_by_key(|s| s.id);
                    summaries
                        .iter()
                        .map(|s| {
                            format!(
                                "[{}] {} \"{}\" — {}",
                                s.id,
                                s.role,
                                s.task,
                                format_status(&s.status)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }
            (Some("stop"), Some(id_str)) => {
                match id_str.parse::<entheai_orchestrator::WorkerId>() {
                    Ok(id) if pool.stop(id) => format!("stopped worker {id}"),
                    Ok(id) => format!("no such worker {id}"),
                    Err(_) => format!("invalid worker id: {id_str}"),
                }
            }
            (Some("debug"), Some(id_str)) => match id_str.parse::<entheai_orchestrator::WorkerId>()
            {
                Ok(id) => match pool.status(id) {
                    None => format!("no such worker {id}"),
                    Some(status) => match pool.output_snapshot(id) {
                        Some(out) => format!("[{id}] {}\n{out}", format_status(&status)),
                        None => format!(
                            "[{id}] {} — still running, no live output tail yet",
                            format_status(&status)
                        ),
                    },
                },
                Err(_) => format!("invalid worker id: {id_str}"),
            },
            _ => "usage: /workers [list | stop <id> | debug <id>]".to_string(),
        },
    };
    app.messages.push(Msg {
        role: Role::Tool,
        text: feedback,
    });
    app.follow = true;
}

/// True when the submitted input is the local `/viz` command (never sent to the
/// agent) — toggles the full-screen swarm view.
fn is_viz_command(text: &str) -> bool {
    text.trim() == "/viz"
}

/// Toggle between the chat and full-screen swarm views in response to `/viz`,
/// echoing the switch into history (mirrors the other local commands).
fn handle_viz_command(app: &mut App, text: &str) {
    app.messages.push(Msg {
        role: Role::User,
        text: text.to_string(),
    });
    app.view = if app.view == ViewMode::Chat {
        ViewMode::Swarm
    } else {
        ViewMode::Chat
    };
    let where_now = match app.view {
        ViewMode::Chat => "chat view",
        ViewMode::Swarm => "swarm view",
    };
    app.messages.push(Msg {
        role: Role::Tool,
        text: format!("◈ switched to {where_now} (Ctrl-V to toggle)"),
    });
    app.follow = true;
}

/// True for `/help` (or its `/?` alias) — prints the command + key reference.
fn is_help_command(text: &str) -> bool {
    let t = text.trim();
    t == "/help" || t == "/?"
}

/// Echo the full slash-command list plus key bindings into history so the whole
/// surface is discoverable without leaving the TUI.
fn handle_help_command(app: &mut App) {
    app.messages.push(Msg {
        role: Role::User,
        text: "/help".to_string(),
    });
    let mut body = String::from("commands");
    for (cmd, desc) in SLASH_COMMANDS {
        body.push_str(&format!("\n  {cmd:<9} {desc}"));
    }
    body.push_str(
        "\nkeys: Enter send · Esc Esc stop run · Ctrl-C ×2 quit · q quit (empty input)\
         \n      Ctrl-V viz · Ctrl-P pause · Ctrl-N next · PgUp/PgDn scroll · Tab complete",
    );
    app.messages.push(Msg {
        role: Role::Tool,
        text: body,
    });
    app.follow = true;
}

/// True for `/clear` (or its `/new` alias) — wipes the conversation for a fresh
/// context. Idle-only (gated in the key handler) so it never races a live run.
fn is_clear_command(text: &str) -> bool {
    let t = text.trim();
    t == "/clear" || t == "/new"
}

/// Drop the whole conversation (and any derived plan/scroll state) so the next
/// message starts from an empty context. The system prompt is untouched.
fn handle_clear_command(app: &mut App) {
    app.messages.clear();
    app.streaming_idx = None;
    app.plan.clear();
    app.scroll = 0;
    app.follow = true;
    app.messages.push(Msg {
        role: Role::Tool,
        text: "◧ conversation cleared — fresh context".to_string(),
    });
}

/// True for `/fanout [on|off]` — toggles swarm fan-out for the next message.
fn is_fanout_command(text: &str) -> bool {
    let t = text.trim();
    t == "/fanout" || t.starts_with("/fanout ")
}

/// Flip (or set) whether submitted messages decompose into parallel coders
/// (`app.fanout`, read by the run path) instead of the single-agent loop.
fn handle_fanout_command(app: &mut App, text: &str) {
    app.messages.push(Msg {
        role: Role::User,
        text: text.to_string(),
    });
    app.fanout = match text.split_whitespace().nth(1) {
        Some("on") => true,
        Some("off") => false,
        _ => !app.fanout, // bare toggle
    };
    let state = if app.fanout {
        "on — messages decompose into parallel coders"
    } else {
        "off — single-agent loop"
    };
    app.messages.push(Msg {
        role: Role::Tool,
        text: format!("⑂ fan-out {state}"),
    });
    app.follow = true;
}

/// True for `/model` — reports the active model (switching needs a restart).
fn is_model_command(text: &str) -> bool {
    text.trim() == "/model"
}

/// Echo the active model label; the agent is built once per session, so
/// switching means relaunching with `--model "<provider>/<model>"`.
fn handle_model_command(app: &mut App) {
    app.messages.push(Msg {
        role: Role::User,
        text: "/model".to_string(),
    });
    app.messages.push(Msg {
        role: Role::Tool,
        text: format!(
            "● model: {} — switch by restarting with --model \"<provider>/<model>\"",
            app.model_label
        ),
    });
    app.follow = true;
}

/// True for `/quit` (or its `/exit` alias) — leaves entheai (like Ctrl-C ×2).
fn is_quit_command(text: &str) -> bool {
    let t = text.trim();
    t == "/quit" || t == "/exit"
}

/// Fold a player event into UI state, echoing noteworthy ones into history.
fn handle_radio_event(app: &mut App, ev: RadioEvent) {
    match ev {
        RadioEvent::Fetching { .. } => {} // already echoed on submit
        RadioEvent::Queued { title } => app.messages.push(Msg {
            role: Role::Tool,
            text: format!("♪ queued: {title}"),
        }),
        RadioEvent::NowPlaying { title } => {
            app.messages.push(Msg {
                role: Role::Tool,
                text: format!("♪ now playing: {title}"),
            });
            app.now_playing = Some(title);
        }
        RadioEvent::Paused => {
            if let Some(t) = &app.now_playing {
                app.now_playing = Some(format!("{} (paused)", t.trim_end_matches(" (paused)")));
            }
        }
        RadioEvent::Resumed => {
            if let Some(t) = &app.now_playing {
                app.now_playing = Some(t.trim_end_matches(" (paused)").to_string());
            }
        }
        RadioEvent::Stopped | RadioEvent::QueueEmpty => app.now_playing = None,
        RadioEvent::Error(e) => app.messages.push(Msg {
            role: Role::Error,
            text: format!("radio: {e}"),
        }),
    }
}

/// Map the display history to provider messages for the next run. Only User and
/// Assistant turns are real conversation; Tool/Error lines are display-only. When
/// `system_prompt` is set, it is pushed first as a system message.
fn build_history(system_prompt: Option<&str>, messages: &[Msg]) -> Vec<ChatMessage> {
    let mut out = Vec::new();
    if let Some(sp) = system_prompt {
        out.push(ChatMessage::system(sp));
    }
    out.extend(messages.iter().filter_map(|m| match m.role {
        Role::User => Some(ChatMessage::user(m.text.clone())),
        Role::Assistant => Some(ChatMessage::assistant(m.text.clone())),
        Role::Tool | Role::Error => None,
    }));
    out
}

/// Hard-wrap `s` to `width` columns (character-based, no word boundaries) so the
/// rendered row count is known exactly for scroll math. Always yields >= 1 row.
fn wrap_str(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(width)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

/// Wrap and style ONE message into its visual rows. Prefixes only the first
/// visual line and honors explicit newlines, padding filled rows to full width.
/// This is the exact per-message body of the old `build_history_lines`, lifted
/// out so [`LineCache`] can re-wrap a single (streaming) message without
/// touching the rest of the scrollback while staying byte-identical to a full
/// rebuild.
fn wrap_message(m: &Msg, width: u16) -> Vec<Line<'static>> {
    let w = width.max(1) as usize;
    let (prefix, style, fill) = m.role.style();
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Prefix only the first visual line; honor explicit newlines in the text.
    let mut first = true;
    for logical in m.text.split('\n') {
        let content = if first {
            format!("{prefix}{logical}")
        } else {
            logical.to_string()
        };
        first = false;
        for row in wrap_str(&content, w) {
            let row = if fill {
                // Pad to full width so the row's background reads as a block.
                let pad = w.saturating_sub(row.chars().count());
                format!("{row}{}", " ".repeat(pad))
            } else {
                row
            };
            lines.push(Line::styled(row, style));
        }
    }
    lines
}

/// Build the fully wrapped, styled history as one `Line` per visual row.
///
/// This is the reference full-rebuild: [`LineCache`] produces byte-identical
/// output incrementally (per message), so this stays the ground truth the cache
/// is tested against. Only the tests need it now that the live path wraps
/// per-message through [`LineCache`], so it is gated to test builds.
#[cfg(test)]
fn build_history_lines(messages: &[Msg], width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for m in messages {
        lines.extend(wrap_message(m, width));
    }
    lines
}

/// Per-message wrap record: the byte length of the source text these lines were
/// wrapped from (the change signal) and where they start in the flattened
/// `lines` buffer. During a turn only the streaming (last) message's `text_len`
/// grows, so only it is re-wrapped.
struct CachedMsg {
    text_len: usize,
    line_start: usize,
}

/// Caches the wrapped, styled history lines **per message**. A streamed token
/// only mutates the last message, so `get_or_build` re-wraps just that one
/// message (O(Δ)) and reuses every finalized message's already-wrapped lines —
/// instead of re-wrapping the whole scrollback on every token (which made a turn
/// O(messages × tokens)). The flattened `lines` buffer is returned by borrow so
/// the draw loop never deep-clones the whole history per frame.
#[derive(Default)]
struct LineCache {
    width: u16,
    per_msg: Vec<CachedMsg>,
    lines: Vec<Line<'static>>,
    rebuilds: usize,
}

impl LineCache {
    fn get_or_build(&mut self, messages: &[Msg], width: u16) -> &[Line<'static>] {
        // A width change invalidates every wrap; start clean.
        if self.width != width {
            self.width = width;
            self.per_msg.clear();
            self.lines.clear();
        }
        // Find the first message whose cached wrap is stale — a message was
        // removed/replaced, or (the mid-turn common case) the last message's text
        // grew. Only integer length compares here; no wrapping.
        let common = messages.len().min(self.per_msg.len());
        let mut diverge = common;
        // `zip` yields exactly `common` pairs (stops at the shorter slice).
        for (i, (cached, msg)) in self.per_msg.iter().zip(messages).enumerate() {
            if cached.text_len != msg.text.len() {
                diverge = i;
                break;
            }
        }
        let mut changed = false;
        // Drop the stale suffix: the changed message and everything after it, or
        // trailing cached messages that no longer exist.
        if diverge < self.per_msg.len() {
            self.lines.truncate(self.per_msg[diverge].line_start);
            self.per_msg.truncate(diverge);
            changed = true;
        }
        // Re-wrap only the missing suffix (the changed message plus any appended
        // messages); every earlier message keeps its already-wrapped lines.
        for msg in &messages[self.per_msg.len()..] {
            let line_start = self.lines.len();
            self.lines.extend(wrap_message(msg, width));
            self.per_msg.push(CachedMsg {
                text_len: msg.text.len(),
                line_start,
            });
            changed = true;
        }
        if changed {
            self.rebuilds += 1;
        }
        &self.lines
    }
}

/// Re-borrow the cache's history lines as a `Text`/`Vec<Line>` that points at
/// the cache's string content instead of deep-cloning it. `Paragraph`/`Text`
/// must own their `Vec<Line>`, but each `Line`/`Span` can borrow its bytes, so
/// this is an O(lines) shallow rebuild with **zero string allocations or byte
/// copies** — replacing the old per-frame deep clone of the entire scrollback.
fn borrow_history<'a>(lines: &'a [Line<'static>]) -> Vec<Line<'a>> {
    lines
        .iter()
        .map(|l| {
            let spans: Vec<Span<'a>> = l
                .spans
                .iter()
                .map(|s| Span::styled(s.content.as_ref(), s.style))
                .collect();
            let mut line = Line::from(spans);
            line.style = l.style;
            line.alignment = l.alignment;
            line
        })
        .collect()
}

/// One-time environment banner for the status bar's second row: the current +
/// starting folder, a stable (hostname-seeded) machine id, and the local IP.
/// Computed ONCE at startup — never per frame.
fn env_status_line(root: &std::path::Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let abbr = |p: &std::path::Path| -> String {
        let s = p.display().to_string();
        match (!home.is_empty(), s.strip_prefix(&home)) {
            (true, Some(rest)) => format!("~{rest}"),
            _ => s,
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| root.to_path_buf());
    let host = env_hostname();
    let mid = seeded_machine_id(&host);
    let ip = primary_ip().unwrap_or_else(|| "offline".to_string());
    format!(
        "📁 {}  ·  ⌂ start {}  ·  🖥 {host}·{mid}  ·  {ip}",
        abbr(&cwd),
        abbr(root)
    )
}

fn env_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// A stable machine id SEEDED from the hostname — FNV-1a → 6 hex chars. Same on
/// this machine every run; no hardware PII.
fn seeded_machine_id(host: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in host.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:06x}", h & 0xff_ffff)
}

/// Primary local IPv4 via the connect-a-UDP-socket trick — sends nothing, needs
/// no crate; `local_addr` reflects the OS-chosen source IP for the route.
fn primary_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    Some(sock.local_addr().ok()?.ip().to_string())
}

/// The two-row status bar: row 1 = `entheai · model · state` (left) + `ctx …/…`
/// (right); row 2 = the env banner (folders, machine id, ip).
fn render_status_bar(frame: &mut Frame, app: &App, env_line: &str, status_area: Rect) {
    frame.render_widget(Paragraph::new(status_line(app)), status_area);
    frame.render_widget(
        Paragraph::new(context_line(app)).alignment(ratatui::layout::Alignment::Right),
        status_area,
    );
    if status_area.height >= 2 {
        let row2 = Rect {
            x: status_area.x,
            y: status_area.y + 1,
            width: status_area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                env_line.to_string(),
                Style::default().add_modifier(Modifier::DIM),
            )),
            row2,
        );
    }
}

fn render(
    frame: &mut Frame,
    app: &App,
    lines: &[Line<'static>],
    scroll: u16,
    plan_rows: u16,
    swarm_rows: u16,
    env_line: &str,
) {
    let area = frame.area();

    // Full-screen swarm view (Ctrl-V / /viz): status bar on top, the swarm
    // canvas filling the content area, and the input box at the bottom. Returns
    // before the normal chat layout.
    if app.view == ViewMode::Swarm {
        let [status_area, main_area, input_area] = Layout::vertical([
            Constraint::Length(STATUS_ROWS),
            Constraint::Min(1),
            Constraint::Length(INPUT_ROWS),
        ])
        .areas(area);
        render_status_bar(frame, app, env_line, status_area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" swarm — Ctrl-V to exit ");
        let inner = block.inner(main_area);
        frame.render_widget(block, main_area);
        entheai_viz::swarm::render(
            &app.swarm,
            inner,
            frame.buffer_mut(),
            ratatui::symbols::Marker::HalfBlock,
            app.spinner_frame as u64,
        );
        render_input(frame, app, input_area);
        render_slash_menu(frame, app, input_area);
        return;
    }

    let [status_area, plan_area, swarm_area, history_area, progress_area, input_area] =
        Layout::vertical([
            Constraint::Length(STATUS_ROWS),
            Constraint::Length(plan_rows),
            Constraint::Length(swarm_rows),
            Constraint::Min(1),
            Constraint::Length(PROGRESS_ROWS),
            Constraint::Length(INPUT_ROWS),
        ])
        .areas(area);

    // Status bar (2 rows): row 1 = entheai · model · state (+ ctx/tokens right);
    // row 2 = the env banner (folders · machine id · ip).
    render_status_bar(frame, app, env_line, status_area);

    // Plan pane: boxless, dim-prefixed rows (one per todo item); collapses to
    // zero height (via `plan_rows`) when there's no live plan.
    if !app.plan.is_empty() {
        let plan = Paragraph::new(plan_lines(&app.plan, plan_area.width));
        frame.render_widget(plan, plan_area);
    }

    // History (pre-wrapped: one `Line` per visual row, so the scroll offset is an
    // exact row index). Slice to the visible viewport BEFORE borrowing — rendering
    // rows [scroll .. scroll+height] with no scroll offset shows the identical
    // window, but makes per-frame work O(viewport) instead of O(entire scrollback)
    // (`borrow_history` was rebuilding every cached row every frame ≈ 60fps).
    let vis_h = history_area.height as usize;
    let start = (scroll as usize).min(lines.len());
    let end = (start + vis_h).min(lines.len());
    frame.render_widget(
        Paragraph::new(borrow_history(&lines[start..end])),
        history_area,
    );

    // Inline swarm pane during a fan-out (collapses to zero height when idle,
    // disabled, or in the full-screen swarm view — see `swarm_rows`).
    if swarm_rows > 0 {
        let block = Block::default().borders(Borders::ALL).title(" swarm ");
        let inner = block.inner(swarm_area);
        frame.render_widget(block, swarm_area);
        entheai_viz::swarm::render(
            &app.swarm,
            inner,
            frame.buffer_mut(),
            ratatui::symbols::Marker::Braille,
            app.spinner_frame as u64,
        );
    }

    // Charm-style live progress line: spinner + current action + elapsed time.
    // Blank when idle so the input box never jumps; the permission modal covers
    // this row visually while awaiting approval, so we just show a static note.
    let progress = match &app.status {
        Status::Working => {
            let elapsed = app.run_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            let label = if app.current_action.starts_with("running ") {
                app.current_action.clone()
            } else {
                format!("{}…", verb_for(app.verb_idx))
            };
            Line::from(vec![
                Span::styled(
                    FRAMES[app.spinner_frame],
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!(
                        "{label} · {elapsed}s · ↓{} tokens",
                        fmt_tokens(app.out_tokens)
                    ),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ])
        }
        Status::AwaitingPermission { .. } => Line::styled(
            "awaiting approval",
            Style::default().add_modifier(Modifier::DIM),
        ),
        Status::Idle => Line::from(""),
    };
    frame.render_widget(Paragraph::new(progress), progress_area);
    // Double-tap hint (e.g. "press Esc again to stop the run"), right-aligned so
    // it sits clear of the left-aligned spinner.
    if let Some(note) = &app.notice {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("{note} "),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(ratatui::layout::Alignment::Right),
            progress_area,
        );
    }

    // Input box + cursor.
    render_input(frame, app, input_area);
    render_slash_menu(frame, app, input_area);

    // Permission modal, centered over history.
    if let Status::AwaitingPermission { tool, args } = &app.status {
        let args = truncate(args, 80);
        let text = format!("allow {tool}({args})?  [y]es · [n]o · [a]llow for session");
        let modal_w = (text.chars().count() as u16 + 4).min(area.width.saturating_sub(2));
        let modal_area = centered_rect(modal_w, 3, area);
        frame.render_widget(Clear, modal_area);
        let modal = Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("permission")
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(modal, modal_area);
    }
}

/// Build the top status bar line: `entheai · <model> · [fan-out ·] <state>
/// [· ♪ track]`. Shared by the chat and full-screen swarm views.
fn status_line(app: &App) -> Line<'static> {
    let state = match &app.status {
        Status::Idle => "idle",
        Status::Working => "working…",
        Status::AwaitingPermission { .. } => "awaiting permission",
    };
    let mut status_spans: Vec<Span<'static>> = vec![
        Span::styled("entheai", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" · "),
        Span::raw(app.model_label.clone()),
    ];
    if app.fanout {
        status_spans.push(Span::raw(" · "));
        status_spans.push(Span::styled("fan-out", Style::default().fg(Color::Magenta)));
    }
    status_spans.push(Span::raw(" · "));
    status_spans.push(Span::styled(state, Style::default().fg(Color::Yellow)));
    if let Some(title) = &app.now_playing {
        status_spans.push(Span::raw(" · "));
        status_spans.push(Span::styled(
            format!("♪ {}", truncate(title, 40)),
            Style::default().fg(Color::Magenta),
        ));
    }
    Line::from(status_spans)
}

/// The model's context-window size in tokens, by model-id substring. Approximate
/// — enough to show "how full is the window". Falls back to 128k.
fn max_context_window(model: &str) -> usize {
    let m = model.to_ascii_lowercase();
    if m.contains("deepseek") {
        131_072 // DeepSeek V3.x — 128k
    } else if m.contains("qwen") {
        32_768
    } else if m.contains("claude") {
        200_000
    } else if m.contains("gemini") {
        1_048_576
    } else {
        128_000 // gpt-4.x / o-series / unknown
    }
}

/// Rough current-context size in tokens: the whole conversation (system prompt +
/// every message) at ~4 chars/token — the same approximation `out_tokens` uses.
fn est_context_tokens(app: &App) -> usize {
    let sys = app.system_prompt.as_deref().map(str::len).unwrap_or(0);
    let msgs: usize = app.messages.iter().map(|m| m.text.len()).sum();
    (sys + msgs) / 4
}

/// Right-aligned top-bar segment: context fill + this run's generated tokens —
/// `ctx ~<cur>/<max> · <pct>% · ↓<out>`. Counts are ~char/4 estimates.
fn context_line(app: &App) -> Line<'static> {
    let cur = est_context_tokens(app);
    let max = max_context_window(&app.model_label);
    let pct = (cur.saturating_mul(100) / max.max(1)).min(999);
    let ctx_color = if pct >= 85 {
        Color::Red
    } else if pct >= 60 {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    Line::from(vec![
        Span::styled(
            format!("ctx ~{}/{} · {pct}%", fmt_tokens(cur), fmt_tokens(max)),
            Style::default().fg(ctx_color),
        ),
        Span::styled(
            format!(" · ↓{}", fmt_tokens(app.out_tokens)),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])
}

/// The local slash commands, surfaced in a live menu when the message box starts
/// with `/` so they're discoverable in the TUI. (name, one-line help + sub-forms).
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "list commands + key bindings"),
    ("/clear", "clear the conversation — fresh context"),
    ("/fanout", "toggle swarm fan-out — on · off"),
    ("/model", "show the active model"),
    (
        "/radio",
        "music — add <url> · pause · next · stop  (Ctrl-P / Ctrl-N)",
    ),
    ("/workers", "fan-out swarm — list · stop <id> · debug <id>"),
    ("/viz", "toggle the full-screen swarm view  (Ctrl-V)"),
    ("/quit", "exit entheai  (Ctrl-C ×2)"),
];

/// Slash commands matching the first token of `input` (the command being typed).
fn slash_matches(input: &str) -> Vec<&'static (&'static str, &'static str)> {
    let token = input.split_whitespace().next().unwrap_or("/");
    SLASH_COMMANDS
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(token) || token.starts_with(cmd))
        .collect()
}

/// Overlay a live command menu just above the input box while the message starts
/// with `/`, filtered by what's typed — the slash commands are TUI-discoverable.
fn render_slash_menu(frame: &mut Frame, app: &App, input_area: Rect) {
    if !app.input.starts_with('/') {
        return;
    }
    let matches = slash_matches(&app.input);
    if matches.is_empty() {
        return;
    }
    let lines: Vec<Line<'static>> = matches
        .iter()
        .map(|(cmd, desc)| {
            Line::from(vec![
                Span::styled(
                    format!(" {cmd} "),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    (*desc).to_string(),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ])
        })
        .collect();
    // Sit just above the input box, bounded by the room above it.
    let h = (lines.len() as u16 + 2).min(input_area.y);
    if h < 3 {
        return;
    }
    let menu_area = Rect {
        x: input_area.x,
        y: input_area.y.saturating_sub(h),
        width: input_area.width,
        height: h,
    };
    frame.render_widget(Clear, menu_area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" commands · Tab completes ")
                .border_style(Style::default().fg(Color::Magenta)),
        ),
        menu_area,
    );
}

/// Draw the bordered input box and place the cursor (cursor hidden while the
/// permission modal is up). Shared by the chat and full-screen swarm views.
fn render_input(frame: &mut Frame, app: &App, input_area: Rect) {
    let input = Paragraph::new(app.input.as_str())
        .block(Block::default().borders(Borders::ALL).title("message"))
        .wrap(Wrap { trim: false });
    frame.render_widget(input, input_area);

    if !matches!(app.status, Status::AwaitingPermission { .. }) {
        let inner_w = input_area.width.saturating_sub(2);
        let cx = (app.input.chars().count() as u16).min(inner_w.saturating_sub(1));
        frame.set_cursor_position(Position::new(input_area.x + 1 + cx, input_area.y + 1));
    }
}

/// Truncate to at most `max` chars, appending an ellipsis when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Truncate a tool call's raw JSON argument string for the inline "⚙
/// name(args)" progress line.
fn truncate_args(args: &str, max: usize) -> String {
    truncate(args, max)
}

/// Take the first line of a (possibly multi-line) tool result and truncate it
/// for the inline "  ↳ result" progress line.
fn first_line_trunc(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    truncate(first, max)
}

/// A `width` x `height` rectangle centered within `area` (clamped to fit).
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_str_hard_wraps_and_never_empty() {
        assert_eq!(wrap_str("", 4), vec![String::new()]);
        assert_eq!(
            wrap_str("abcdef", 4),
            vec!["abcd".to_string(), "ef".to_string()]
        );
        assert_eq!(wrap_str("abc", 0), vec!["abc".to_string()]);
    }

    #[test]
    fn slash_matches_filters_by_typed_token() {
        assert_eq!(slash_matches("/").len(), SLASH_COMMANDS.len()); // lists all
        let w = slash_matches("/w");
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0, "/workers");
        // A full command (even with args) still matches itself.
        let r = slash_matches("/radio add http://x");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "/radio");
        assert!(slash_matches("/nope").is_empty());
    }

    /// Minimal idle `App` for exercising command handlers and the key handler.
    fn test_app() -> App {
        App {
            messages: Vec::new(),
            input: String::new(),
            status: Status::Idle,
            scroll: 0,
            follow: true,
            model_label: "deepseek/deepseek-chat".into(),
            pending_permission: None,
            run_started: None,
            spinner_frame: 0,
            current_action: String::new(),
            now_playing: None,
            fanout: false,
            worker_pool: None,
            system_prompt: None,
            streaming_idx: None,
            out_tokens: 0,
            verb_idx: 0,
            plan: Vec::new(),
            swarm: entheai_viz::SwarmModel::new(),
            view: ViewMode::Chat,
            viz_swarm: false,
            last_esc: None,
            last_ctrl_c: None,
            notice: None,
        }
    }

    #[test]
    fn new_command_predicates_match_their_verbs() {
        assert!(is_help_command("/help") && is_help_command("/?"));
        assert!(is_clear_command("/clear") && is_clear_command("/new"));
        assert!(is_fanout_command("/fanout") && is_fanout_command("/fanout on"));
        assert!(is_model_command("/model"));
        assert!(is_quit_command("/quit") && is_quit_command("/exit"));
        // Lookalikes and non-commands stay out.
        assert!(!is_clear_command("/clearx"));
        assert!(!is_quit_command("quit"));
        assert!(!is_fanout_command("/fan"));
    }

    #[test]
    fn fanout_command_toggles_and_sets() {
        let mut app = test_app();
        assert!(!app.fanout);
        handle_fanout_command(&mut app, "/fanout");
        assert!(app.fanout, "bare /fanout toggles on");
        handle_fanout_command(&mut app, "/fanout");
        assert!(!app.fanout, "bare /fanout toggles back off");
        handle_fanout_command(&mut app, "/fanout on");
        assert!(app.fanout, "/fanout on forces on");
        handle_fanout_command(&mut app, "/fanout off");
        assert!(!app.fanout, "/fanout off forces off");
    }

    #[test]
    fn clear_command_empties_history_but_keeps_system_prompt() {
        let mut app = test_app();
        app.system_prompt = Some("skills advertisement".into());
        app.messages.push(Msg {
            role: Role::User,
            text: "hi".into(),
        });
        app.messages.push(Msg {
            role: Role::Assistant,
            text: "hello".into(),
        });
        app.scroll = 9;
        handle_clear_command(&mut app);
        // Only the confirmation line remains; the prior turns are gone.
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0].role, Role::Tool));
        assert_eq!(app.scroll, 0);
        assert_eq!(app.system_prompt.as_deref(), Some("skills advertisement"));
    }

    #[test]
    fn double_esc_stops_a_running_task() {
        let mut app = test_app();
        app.status = Status::Working;
        // First Esc arms + hints, no action yet.
        let a1 = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(a1, Action::None));
        assert!(app.notice.is_some());
        // Second Esc within the window cancels the run.
        let a2 = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(a2, Action::CancelRun));
    }

    #[test]
    fn intervening_key_breaks_the_double_tap_chain() {
        let mut app = test_app();
        app.status = Status::Working;
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        // A normal keystroke resets the chain...
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        // ...so the next Esc is a fresh first-tap, not a cancel.
        let a = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(a, Action::None));
    }

    #[test]
    fn double_ctrl_c_quits() {
        let mut app = test_app();
        let ctrl = KeyModifiers::CONTROL;
        let a1 = handle_key(&mut app, KeyEvent::new(KeyCode::Char('c'), ctrl));
        assert!(matches!(a1, Action::None));
        assert!(app.notice.is_some());
        let a2 = handle_key(&mut app, KeyEvent::new(KeyCode::Char('c'), ctrl));
        assert!(matches!(a2, Action::Quit));
    }

    #[test]
    fn build_history_skips_tool_and_error() {
        let messages = vec![
            Msg {
                role: Role::User,
                text: "hi".into(),
            },
            Msg {
                role: Role::Tool,
                text: "ran".into(),
            },
            Msg {
                role: Role::Assistant,
                text: "yo".into(),
            },
            Msg {
                role: Role::Error,
                text: "boom".into(),
            },
        ];
        let hist = build_history(None, &messages);
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].role, "user");
        assert_eq!(hist[1].role, "assistant");
    }

    #[test]
    fn history_lines_prefix_first_row_only() {
        let messages = vec![Msg {
            role: Role::User,
            text: "hello".into(),
        }];
        // Wide enough that "you> hello" stays one row.
        let lines = build_history_lines(&messages, 80);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn truncate_adds_ellipsis_when_long() {
        assert_eq!(truncate("short", 80), "short");
        let t = truncate(&"x".repeat(200), 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn centered_rect_is_centered_and_clamped() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let r = centered_rect(20, 3, area);
        assert_eq!(r.width, 20);
        assert_eq!(r.height, 3);
        assert_eq!(r.x, 40);
        // Clamp oversized requests to the area.
        let big = centered_rect(200, 100, area);
        assert_eq!(big.width, 100);
        assert_eq!(big.height, 40);
    }

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

    #[test]
    fn plan_lines_markers_and_empty() {
        use entheai_tools::todo::{TodoItem, TodoStatus};
        assert!(plan_lines(&[], 40).is_empty()); // empty -> no rows
        let plan = vec![
            TodoItem {
                text: "read".into(),
                status: TodoStatus::Done,
            },
            TodoItem {
                text: "map".into(),
                status: TodoStatus::InProgress,
            },
            TodoItem {
                text: "add".into(),
                status: TodoStatus::Pending,
            },
        ];
        let lines = plan_lines(&plan, 40);
        assert_eq!(lines.len(), 3);
        // render to strings to check markers
        let s: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        assert!(s[0].starts_with('✓'));
        assert!(s[1].starts_with('◐'));
        assert!(s[2].starts_with('◻'));
    }

    #[test]
    fn line_cache_rebuilds_on_change_only() {
        let mut c = LineCache::default();
        let mut msgs = vec![Msg {
            role: Role::User,
            text: "hi".into(),
        }];
        let a = c.get_or_build(&msgs, 40).len();
        let b = c.get_or_build(&msgs, 40).len(); // same key -> no rebuild
        assert_eq!(c.rebuilds, 1);
        assert_eq!(a, b);
        msgs.push(Msg {
            role: Role::Assistant,
            text: "yo".into(),
        });
        c.get_or_build(&msgs, 40); // changed -> rebuild
        assert_eq!(c.rebuilds, 2);
    }

    #[test]
    fn line_cache_matches_full_rebuild_while_streaming() {
        let width = 24u16;
        let mut msgs = vec![
            Msg {
                role: Role::User,
                text: "hello there dear friend".into(),
            },
            Msg {
                role: Role::Tool,
                text: "ran a command\nwith two logical lines".into(),
            },
            // The in-progress streaming bubble starts empty.
            Msg {
                role: Role::Assistant,
                text: String::new(),
            },
        ];
        let mut cache = LineCache::default();

        // Initial incremental build equals a naive full re-wrap.
        assert_eq!(
            cache.get_or_build(&msgs, width),
            build_history_lines(&msgs, width).as_slice()
        );

        // Stream tokens into the last message. After EVERY token the incremental
        // cache must still equal a from-scratch re-wrap of every message.
        for tok in [
            "The ",
            "quick ",
            "brown fox ",
            "jumps over the ",
            "very lazy sleeping dog",
        ] {
            let last = msgs.len() - 1;
            msgs[last].text.push_str(tok);
            let got = cache.get_or_build(&msgs, width).to_vec();
            assert_eq!(got, build_history_lines(&msgs, width));
        }

        // O(Δ) contract: 1 initial build + 5 single-message re-wraps, NOT a whole
        // -history rebuild per token.
        assert_eq!(cache.rebuilds, 6);

        // A width change invalidates and rebuilds against the new wrap width.
        let narrow = 9u16;
        assert_eq!(
            cache.get_or_build(&msgs, narrow),
            build_history_lines(&msgs, narrow).as_slice()
        );
        // Re-querying at the same (new) width does no extra work.
        let before = cache.rebuilds;
        cache.get_or_build(&msgs, narrow);
        assert_eq!(cache.rebuilds, before);

        // Finalize the turn and append a fresh message: output stays correct and
        // only the appended message is wrapped.
        let before = cache.rebuilds;
        msgs.push(Msg {
            role: Role::User,
            text: "and the next question".into(),
        });
        assert_eq!(
            cache.get_or_build(&msgs, narrow),
            build_history_lines(&msgs, narrow).as_slice()
        );
        assert_eq!(cache.rebuilds, before + 1);
    }

    #[test]
    fn radio_command_detection() {
        assert!(is_radio_command("/radio"));
        assert!(is_radio_command("/radio https://youtu.be/x"));
        assert!(is_radio_command("  /radio pause"));
        assert!(!is_radio_command("/radiohead"));
        assert!(!is_radio_command("play some music"));
    }

    #[test]
    fn workers_command_detection() {
        assert!(is_workers_command("/workers"));
        assert!(is_workers_command("/workers list"));
        assert!(is_workers_command("  /workers stop 0"));
        assert!(!is_workers_command("/workersomething"));
        assert!(!is_workers_command("list workers"));
    }

    #[test]
    fn workers_command_submits_even_while_working() {
        let mut app = App {
            messages: Vec::new(),
            input: "/workers list".to_string(),
            status: Status::Working,
            scroll: 0,
            follow: true,
            model_label: "m".into(),
            pending_permission: None,
            run_started: None,
            spinner_frame: 0,
            current_action: String::new(),
            now_playing: None,
            fanout: true,
            worker_pool: None,
            system_prompt: None,
            streaming_idx: None,
            out_tokens: 0,
            verb_idx: 0,
            plan: Vec::new(),
            swarm: entheai_viz::SwarmModel::new(),
            view: ViewMode::Chat,
            viz_swarm: false,
            last_esc: None,
            last_ctrl_c: None,
            notice: None,
        };
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let action = handle_key(&mut app, key);
        match action {
            Action::Submit(text) => assert_eq!(text, "/workers list"),
            _ => panic!("expected Action::Submit while Working for a /workers command"),
        }
    }

    #[test]
    fn plain_message_does_not_submit_while_working() {
        let mut app = App {
            messages: Vec::new(),
            input: "hello agent".to_string(),
            status: Status::Working,
            scroll: 0,
            follow: true,
            model_label: "m".into(),
            pending_permission: None,
            run_started: None,
            spinner_frame: 0,
            current_action: String::new(),
            now_playing: None,
            fanout: false,
            worker_pool: None,
            system_prompt: None,
            streaming_idx: None,
            out_tokens: 0,
            verb_idx: 0,
            plan: Vec::new(),
            swarm: entheai_viz::SwarmModel::new(),
            view: ViewMode::Chat,
            viz_swarm: false,
            last_esc: None,
            last_ctrl_c: None,
            notice: None,
        };
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let action = handle_key(&mut app, key);
        assert!(matches!(action, Action::None));
        assert_eq!(app.input, "hello agent"); // untouched — not submitted, not cleared
    }

    #[test]
    fn at_file_reference_survives_submit_unmodified() {
        let mut app = App {
            messages: Vec::new(),
            input: "@{crates/tui/src/lib.rs} fix the input handler".to_string(),
            status: Status::Idle,
            scroll: 0,
            follow: true,
            model_label: "m".into(),
            pending_permission: None,
            run_started: None,
            spinner_frame: 0,
            current_action: String::new(),
            now_playing: None,
            fanout: true,
            worker_pool: None,
            system_prompt: None,
            streaming_idx: None,
            out_tokens: 0,
            verb_idx: 0,
            plan: Vec::new(),
            swarm: entheai_viz::SwarmModel::new(),
            view: ViewMode::Chat,
            viz_swarm: false,
            last_esc: None,
            last_ctrl_c: None,
            notice: None,
        };
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let action = handle_key(&mut app, key);
        match action {
            Action::Submit(text) => {
                assert_eq!(text, "@{crates/tui/src/lib.rs} fix the input handler")
            }
            _ => panic!("expected Action::Submit for an idle message containing @{{file}}"),
        }
    }

    #[test]
    fn format_status_describes_each_variant() {
        use entheai_orchestrator::WorkerStatus;
        assert_eq!(format_status(&WorkerStatus::Queued), "queued");
        assert!(format_status(&WorkerStatus::Running {
            started_at: std::time::Instant::now(),
        })
        .starts_with("running "));
        assert_eq!(format_status(&WorkerStatus::Done), "done");
        assert_eq!(format_status(&WorkerStatus::TimedOut), "timed out");
        assert_eq!(format_status(&WorkerStatus::Killed), "killed");
    }

    #[test]
    fn plan_rows_uses_configured_cap() {
        assert_eq!(plan_rows_for(20, 5), 5); // 20 items clamped to cap 5
        assert_eq!(plan_rows_for(0, 8), 0); // empty collapses
        assert_eq!(plan_rows_for(3, 8), 3); // under cap
    }

    #[test]
    fn swarm_pane_collapses_when_idle() {
        let m = entheai_viz::SwarmModel::new(); // Idle, empty
        assert_eq!(swarm_rows_for(true, &m, 8), 0);
        let mut active = entheai_viz::SwarmModel::new();
        active.decompose(&[("a".into(), "t".into())]);
        assert_eq!(swarm_rows_for(true, &active, 8), 3); // 1 node + 2 border
        assert_eq!(swarm_rows_for(false, &active, 8), 0); // disabled → collapsed
    }

    #[test]
    fn swarm_pane_clamps_at_cap() {
        let mut m = entheai_viz::SwarmModel::new();
        let tasks: Vec<(String, String)> = (0..12).map(|i| (format!("r{i}"), "t".into())).collect();
        m.decompose(&tasks);
        assert_eq!(swarm_rows_for(true, &m, 8), 8, "12 nodes clamp to the cap");
    }

    #[test]
    fn swarm_pane_collapses_after_done() {
        let mut m = entheai_viz::SwarmModel::new();
        m.decompose(&[("a".into(), "t".into())]);
        assert!(swarm_rows_for(true, &m, 8) > 0, "active during the run");
        m.done(None, 1, 0);
        assert_eq!(
            swarm_rows_for(true, &m, 8),
            0,
            "collapses once the run is Done"
        );
    }

    #[test]
    fn workers_command_reports_no_fanout_running_when_pool_is_none() {
        let mut app = App {
            messages: Vec::new(),
            input: String::new(),
            status: Status::Idle,
            scroll: 0,
            follow: true,
            model_label: "m".into(),
            pending_permission: None,
            run_started: None,
            spinner_frame: 0,
            current_action: String::new(),
            now_playing: None,
            fanout: true,
            worker_pool: None,
            system_prompt: None,
            streaming_idx: None,
            out_tokens: 0,
            verb_idx: 0,
            plan: Vec::new(),
            swarm: entheai_viz::SwarmModel::new(),
            view: ViewMode::Chat,
            viz_swarm: false,
            last_esc: None,
            last_ctrl_c: None,
            notice: None,
        };
        handle_workers_command(&mut app, "/workers list");
        assert!(app
            .messages
            .last()
            .expect("feedback message")
            .text
            .contains("no fan-out running"));
    }

    #[test]
    fn radio_events_update_now_playing() {
        let mut app = App {
            messages: Vec::new(),
            input: String::new(),
            status: Status::Idle,
            scroll: 0,
            follow: true,
            model_label: "m".into(),
            pending_permission: None,
            run_started: None,
            spinner_frame: 0,
            current_action: String::new(),
            now_playing: None,
            fanout: false,
            worker_pool: None,
            system_prompt: None,
            streaming_idx: None,
            out_tokens: 0,
            verb_idx: 0,
            plan: Vec::new(),
            swarm: entheai_viz::SwarmModel::new(),
            view: ViewMode::Chat,
            viz_swarm: false,
            last_esc: None,
            last_ctrl_c: None,
            notice: None,
        };
        handle_radio_event(
            &mut app,
            RadioEvent::NowPlaying {
                title: "Song".into(),
            },
        );
        assert_eq!(app.now_playing.as_deref(), Some("Song"));
        handle_radio_event(&mut app, RadioEvent::Paused);
        assert_eq!(app.now_playing.as_deref(), Some("Song (paused)"));
        handle_radio_event(&mut app, RadioEvent::Resumed);
        assert_eq!(app.now_playing.as_deref(), Some("Song"));
        handle_radio_event(&mut app, RadioEvent::Stopped);
        assert!(app.now_playing.is_none());
        assert!(app.messages.iter().any(|m| m.text.contains("now playing")));
    }
}
