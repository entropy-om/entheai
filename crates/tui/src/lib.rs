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
    /// Optional system prompt (e.g. skills advertisement) prepended to the
    /// conversation history sent on each single-agent run.
    system_prompt: Option<String>,
    /// Index into `messages` of the assistant bubble currently being streamed
    /// into by live `AgentEvent::Token`s, if any.
    streaming_idx: Option<usize>,
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
}

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
        system_prompt,
        streaming_idx: None,
    };

    // Background music player (yt-dlp + rodio); one per TUI session.
    let mut radio = Radio::spawn(Radio::default_cache_dir()).expect("spawn radio thread");

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(90));

    loop {
        // Clamp scroll against the current terminal size before drawing.
        let size = terminal.size()?;
        let history_height = size
            .height
            .saturating_sub(STATUS_ROWS + PROGRESS_ROWS + INPUT_ROWS);
        let lines = build_history_lines(&app.messages, size.width);
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
        terminal.draw(|frame| render(frame, &app, lines, scroll))?;

        tokio::select! {
            maybe_event = events.next() => {
                let Some(ev) = maybe_event else { break };
                let ev = match ev {
                    Ok(ev) => ev,
                    Err(_) => break,
                };
                if let Event::Key(key) = ev {
                    match handle_key(&mut app, key) {
                        Action::Quit => break,
                        Action::None => {}
                        Action::RadioToggle => radio.send(RadioCommand::TogglePause),
                        Action::RadioNext => radio.send(RadioCommand::Next),
                        Action::Submit(text) if is_radio_command(&text) => {
                            handle_radio_command(&mut app, &radio, &text);
                        }
                        Action::Submit(text) => {
                            app.messages.push(Msg { role: Role::User, text: text.clone() });
                            app.status = Status::Working;
                            if let Some(ref tx) = companion_tx {
                                let _ = tx.send(StateChange::working());
                            }
                            app.follow = true;
                            app.current_action = "thinking".to_string();
                            app.run_started = Some(Instant::now());

                            if fanout {
                                let config = Arc::clone(&config);
                                let root = root.clone();
                                let result_tx = result_tx.clone();
                                let (ftx, frx) =
                                    mpsc::unbounded_channel::<entheai_orchestrator::FanoutEvent>();
                                fanout_rx = Some(frx);
                                tokio::spawn(async move {
                                    let res =
                                        entheai_orchestrator::run_fanout(&config, &root, &text, Some(ftx))
                                            .await;
                                    let _ = result_tx.send(res.map_err(|e| e.to_string())).await;
                                });
                            } else {
                                let history = build_history(app.system_prompt.as_deref(), &app.messages);

                                let agent = Arc::clone(&agent);
                                let registry = Arc::clone(&registry);
                                let policy = Arc::clone(&policy);
                                let perm_tx = perm_tx.clone();
                                let result_tx = result_tx.clone();
                                let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();
                                events_rx = Some(event_rx);
                                tokio::spawn(async move {
                                    let mut prompter = TuiPrompter { tx: perm_tx };
                                    let res = agent
                                        .run_task(history, &registry, &policy, &mut prompter, Some(event_tx))
                                        .await;
                                    let _ = result_tx.send(res.map_err(|e| e.to_string())).await;
                                });
                            }
                        }
                    }
                }
            }
            Some(req) = perm_rx.recv() => {
                app.pending_permission = Some(req.respond);
                if let Some(ref tx) = companion_tx {
                    let _ = tx.send(StateChange::permission_pending(&req.tool, &req.args));
                }
                app.status = Status::AwaitingPermission { tool: req.tool, args: req.args };
            }
            Some(result) = result_rx.recv() => {
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
                app.streaming_idx = None;
            }
            maybe_progress = async {
                match events_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
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
                        app.messages[idx].text.push_str(&t);
                    }
                    Some(AgentEvent::ToolStarted { name, args }) => {
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
                match maybe_fanout {
                    Some(entheai_orchestrator::FanoutEvent::Fallback) => {
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: "⋔ not a git repo — read-only fan-out".to_string(),
                        });
                    }
                    Some(entheai_orchestrator::FanoutEvent::Decomposed { tasks }) => {
                        let count = tasks.len();
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("◇ decomposed into {count} sub-task(s)"),
                        });
                        app.current_action = "fanning out".to_string();
                    }
                    Some(entheai_orchestrator::FanoutEvent::CoderStarted { index, role, task }) => {
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("▸ [{role} #{index}] {}", truncate(&task, 80)),
                        });
                        app.current_action = "running coders".to_string();
                    }
                    Some(entheai_orchestrator::FanoutEvent::CoderFinished { index, committed: _, status }) => {
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("  #{index}: {status}"),
                        });
                    }
                    Some(entheai_orchestrator::FanoutEvent::Integrating { branches }) => {
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!("⧉ integrating {branches} branch(es)…"),
                        });
                        app.current_action = "integrating".to_string();
                    }
                    Some(entheai_orchestrator::FanoutEvent::Done { integration_branch, merged, conflicted }) => {
                        app.messages.push(Msg {
                            role: Role::Tool,
                            text: format!(
                                "◆ done — {merged} merged, {conflicted} conflicted{}",
                                integration_branch.map(|b| format!(" · branch {b}")).unwrap_or_default()
                            ),
                        });
                    }
                    None => fanout_rx = None, // sender dropped -> run finished
                }
            }
            Some(rev) = radio.next_event() => {
                handle_radio_event(&mut app, rev);
            }
            _ = ticker.tick() => {
                if matches!(app.status, Status::Working) {
                    app.spinner_frame = (app.spinner_frame + 1) % FRAMES.len();
                }
            }
        }
    }

    Ok(())
}

const STATUS_ROWS: u16 = 1;
const PROGRESS_ROWS: u16 = 1;
const INPUT_ROWS: u16 = 3;

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

    match key.code {
        // Radio transport keys work whether idle or mid-run.
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Action::RadioToggle;
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Action::RadioNext;
        }
        // Ctrl-C / Esc quit only when idle (never mid-run or mid-modal).
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if idle {
                return Action::Quit;
            }
        }
        KeyCode::Esc if idle => return Action::Quit,
        // Bare `q` quits only when idle AND the input is empty, so a chat message
        // can still contain the letter q.
        KeyCode::Char('q') if idle && app.input.is_empty() => return Action::Quit,
        KeyCode::Enter => {
            if idle && !app.input.trim().is_empty() {
                return Action::Submit(std::mem::take(&mut app.input));
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

/// Build the fully wrapped, styled history as one `Line` per visual row.
fn build_history_lines(messages: &[Msg], width: u16) -> Vec<Line<'static>> {
    let w = width.max(1) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for m in messages {
        let (prefix, style, fill) = m.role.style();
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
    }
    lines
}

fn render(frame: &mut Frame, app: &App, lines: Vec<Line<'static>>, scroll: u16) {
    let area = frame.area();
    let [status_area, history_area, progress_area, input_area] = Layout::vertical([
        Constraint::Length(STATUS_ROWS),
        Constraint::Min(1),
        Constraint::Length(PROGRESS_ROWS),
        Constraint::Length(INPUT_ROWS),
    ])
    .areas(area);

    // Status bar: entheai · <model> · <state>
    let state = match &app.status {
        Status::Idle => "idle",
        Status::Working => "working…",
        Status::AwaitingPermission { .. } => "awaiting permission",
    };
    let mut status_spans: Vec<Span> = vec![
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
    let status = Line::from(status_spans);
    frame.render_widget(Paragraph::new(status), status_area);

    // History (pre-wrapped, so scroll offset is exact).
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), history_area);

    // Charm-style live progress line: spinner + current action + elapsed time.
    // Blank when idle so the input box never jumps; the permission modal covers
    // this row visually while awaiting approval, so we just show a static note.
    let progress = match &app.status {
        Status::Working => {
            let elapsed = app.run_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            Line::from(vec![
                Span::styled(
                    FRAMES[app.spinner_frame],
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{} · {elapsed}s", app.current_action),
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

    // Input box.
    let input = Paragraph::new(app.input.as_str())
        .block(Block::default().borders(Borders::ALL).title("message"))
        .wrap(Wrap { trim: false });
    frame.render_widget(input, input_area);

    // Cursor at the end of the input (only when the user can type).
    if !matches!(app.status, Status::AwaitingPermission { .. }) {
        let inner_w = input_area.width.saturating_sub(2);
        let cx = (app.input.chars().count() as u16).min(inner_w.saturating_sub(1));
        frame.set_cursor_position(Position::new(input_area.x + 1 + cx, input_area.y + 1));
    }

    // Permission modal, centered over history.
    if let Status::AwaitingPermission { tool, args } = &app.status {
        let args = truncate(args, 80);
        let text = format!("allow {tool}({args})?  [y]es / [n]o");
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
    fn radio_command_detection() {
        assert!(is_radio_command("/radio"));
        assert!(is_radio_command("/radio https://youtu.be/x"));
        assert!(is_radio_command("  /radio pause"));
        assert!(!is_radio_command("/radiohead"));
        assert!(!is_radio_command("play some music"));
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
            system_prompt: None,
            streaming_idx: None,
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
