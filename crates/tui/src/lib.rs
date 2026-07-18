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

use entheai_core::{Agent, AgentEvent, CoreError};
use entheai_permission::{Policy, Prompter};
use entheai_providers::{ChatMessage, Provider};
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
    /// Reserved for a future inline tool-progress feature; styling exists (dim)
    /// but v1 never emits it (no inline tool list).
    #[allow(dead_code)]
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
    respond: oneshot::Sender<bool>,
}

/// `Prompter` impl used inside spawned run tasks: forwards each `confirm` to the
/// UI thread and awaits the user's yes/no over a oneshot.
struct TuiPrompter {
    tx: mpsc::Sender<PermissionRequest>,
}

#[async_trait]
impl Prompter for TuiPrompter {
    async fn confirm(&mut self, tool_name: &str, args_summary: &str) -> bool {
        let (respond, rx) = oneshot::channel();
        let req = PermissionRequest {
            tool: tool_name.to_string(),
            args: args_summary.to_string(),
            respond,
        };
        if self.tx.send(req).await.is_err() {
            return false; // UI gone -> deny
        }
        rx.await.unwrap_or(false) // UI dropped the responder -> deny
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
    pending_permission: Option<oneshot::Sender<bool>>,
    /// When the current run started; `None` while idle. Drives the elapsed-time
    /// display in the live progress line.
    run_started: Option<Instant>,
    /// Current frame index into [`FRAMES`] for the progress-line spinner.
    spinner_frame: usize,
    /// Human-readable description of what the agent is doing right now, e.g.
    /// "thinking" or "running read_file".
    current_action: String,
}

/// What a key press asked the loop to do.
enum Action {
    None,
    Quit,
    Submit(String),
}

/// Run the interactive TUI. Sets up the terminal, runs the event loop, and
/// always restores the terminal on exit (raw mode + alternate screen), even on
/// error, via [`TerminalGuard`].
pub async fn run<P: Provider + 'static>(
    agent: Agent<P>,
    registry: ToolRegistry,
    policy: Policy,
    model_label: String,
) -> anyhow::Result<()> {
    let mut terminal = init_terminal()?;
    let guard = TerminalGuard;
    let result = event_loop(&mut terminal, agent, registry, policy, model_label).await;
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

async fn event_loop<P: Provider + 'static>(
    terminal: &mut Terminal<Backend>,
    agent: Agent<P>,
    registry: ToolRegistry,
    policy: Policy,
    model_label: String,
) -> anyhow::Result<()> {
    // Arc so each spawned run task can share the agent/registry/policy.
    let agent = Arc::new(agent);
    let registry = Arc::new(registry);
    let policy = Arc::new(policy);

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionRequest>(8);
    let (result_tx, mut result_rx) = mpsc::channel::<Result<String, CoreError>>(8);
    // Receiver for the currently running task's progress events, if any. Set on
    // submit, torn down when the run's sender is dropped (channel closes) or the
    // result arrives.
    let mut events_rx: Option<mpsc::UnboundedReceiver<AgentEvent>> = None;

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
    };

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
                        Action::Submit(text) => {
                            app.messages.push(Msg { role: Role::User, text });
                            app.status = Status::Working;
                            app.follow = true;
                            app.current_action = "thinking".to_string();
                            app.run_started = Some(Instant::now());
                            let history = build_history(&app.messages);

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
                                let _ = result_tx.send(res).await;
                            });
                        }
                    }
                }
            }
            Some(req) = perm_rx.recv() => {
                app.pending_permission = Some(req.respond);
                app.status = Status::AwaitingPermission { tool: req.tool, args: req.args };
            }
            Some(result) = result_rx.recv() => {
                match result {
                    Ok(answer) => app.messages.push(Msg { role: Role::Assistant, text: answer }),
                    Err(err) => app.messages.push(Msg { role: Role::Error, text: format!("{err}") }),
                }
                app.status = Status::Idle;
                app.follow = true;
                app.run_started = None;
                events_rx = None;
            }
            maybe_progress = async {
                match events_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match maybe_progress {
                    Some(AgentEvent::Thinking) => app.current_action = "thinking".to_string(),
                    Some(AgentEvent::ToolStarted { name }) => {
                        app.current_action = format!("running {name}");
                    }
                    Some(AgentEvent::ToolFinished { .. }) => {
                        app.current_action = "thinking".to_string();
                    }
                    None => events_rx = None, // sender dropped -> run finished
                }
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
                    let _ = tx.send(true);
                }
                app.status = Status::Working;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                if let Some(tx) = app.pending_permission.take() {
                    let _ = tx.send(false);
                }
                app.status = Status::Working;
            }
            _ => {}
        }
        return Action::None;
    }

    let idle = matches!(app.status, Status::Idle);

    match key.code {
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

/// Map the display history to provider messages for the next run. Only User and
/// Assistant turns are real conversation; Tool/Error lines are display-only.
fn build_history(messages: &[Msg]) -> Vec<ChatMessage> {
    messages
        .iter()
        .filter_map(|m| match m.role {
            Role::User => Some(ChatMessage::user(m.text.clone())),
            Role::Assistant => Some(ChatMessage::assistant(m.text.clone())),
            Role::Tool | Role::Error => None,
        })
        .collect()
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
    let status = Line::from(vec![
        Span::styled("entheai", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" · "),
        Span::raw(app.model_label.clone()),
        Span::raw(" · "),
        Span::styled(state, Style::default().fg(Color::Yellow)),
    ]);
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
        let hist = build_history(&messages);
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
}
