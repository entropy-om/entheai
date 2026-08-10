//! `tantra-agent` CLI — the primary surface for controlling the tantric board
//! (https://mlxquantlovefrom.com/board), a GitHub-issues-backed kanban.
//!
//! ```text
//! tantra-agent list                 # all lanes + cards (read)
//! tantra-agent add --title T --lane tantra
//! tantra-agent move --number 3 --lane burning
//! tantra-agent todo list | todo add <text>
//! tantra-agent summary today <text...>
//! tantra-agent whoami
//! tantra-agent agent "move card 3 to burning"
//! ```
//!
//! Writes require one of the three collaborator tokens in the environment:
//! `TANTRIC_TOKEN_PETER` (peterlodri-sec), `TANTRIC_TOKEN_8BIT`
//! (8bit-wraith), `TANTRIC_TOKEN_SG` (standardgalactic).

use std::collections::BTreeMap;

use anyhow::anyhow;
use clap::{Parser, Subcommand};
use tantra_agent::agent::run_agent;
use tantra_agent::board::{self, daily_title, todo_title, Board, Collaborator, GhIssue, Lane};

#[derive(Parser)]
#[command(
    name = "tantra-agent",
    version,
    about = "Board-control agent for the tantric board (mlxquantlovefrom.com)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show all lanes + cards (read; GitHub issues via the repo's public read).
    List,
    /// Create a card (issue with the lane label). Needs a token.
    Add {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "tantra")]
        lane: String,
    },
    /// Move a card to a lane (update labels). Needs a token.
    Move {
        #[arg(long)]
        number: u64,
        #[arg(long)]
        lane: String,
    },
    /// Your own todo list (per-collaborator issue "todo: <login>").
    Todo {
        #[command(subcommand)]
        action: TodoAction,
    },
    /// Your daily summary (per-collaborator issue "daily: <login>", append).
    Summary {
        #[command(subcommand)]
        action: SummaryAction,
    },
    /// Which collaborator token is active (from env).
    Whoami,
    /// Run the adk-rust agent: one prompt, the tantric tools wired
    /// (thin wrapper — the CLI is the primary surface).
    Agent {
        #[arg(required = true, num_args = 1..)]
        prompt: Vec<String>,
    },
}

#[derive(Subcommand)]
enum TodoAction {
    /// List your todo items.
    List,
    /// Append a todo item to your todo issue (creates it on first use).
    Add {
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
}

#[derive(Subcommand)]
enum SummaryAction {
    /// Append today's summary to your daily issue (creates it on first use).
    Today {
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let cli = Cli::parse();

    match cli.command {
        Command::List => cmd_list(&Board::from_env()?).await,
        Command::Add { title, lane } => {
            let lane = parse_lane(&lane)?;
            let board = Board::require_token()?;
            let issue = board.create_issue(&title, "", &[lane.label()]).await?;
            println!(
                "created card #{issue_number} in [{lane}]: {title}",
                issue_number = issue.number
            );
            Ok(())
        }
        Command::Move { number, lane } => {
            let lane = parse_lane(&lane)?;
            let board = Board::require_token()?;
            let issue = board.update_labels(number, &[lane.label()]).await?;
            println!("moved card #{number} -> [{lane}]");
            println!("  {title}", title = issue.title);
            Ok(())
        }
        Command::Todo { action } => match action {
            TodoAction::List => cmd_todo_list().await,
            TodoAction::Add { text } => cmd_todo_add(&text.join(" ")).await,
        },
        Command::Summary { action } => match action {
            SummaryAction::Today { text } => cmd_summary_today(&text.join(" ")).await,
        },
        Command::Whoami => cmd_whoami(),
        Command::Agent { prompt } => {
            let answer = run_agent(&prompt.join(" ")).await?;
            println!("{answer}");
            Ok(())
        }
    }
}

fn parse_lane(s: &str) -> anyhow::Result<Lane> {
    Lane::parse(s)
        .ok_or_else(|| anyhow!("unknown lane {s:?} — expected backlog|burning|tantra|done"))
}

fn current_collaborator() -> anyhow::Result<Collaborator> {
    board::require_token()
}

/// `list` — group open issues into the four lanes, then todo/daily scratch.
async fn cmd_list(board: &Board) -> anyhow::Result<()> {
    let issues = board.list_issues().await?;
    let cards: Vec<&GhIssue> = issues
        .iter()
        .filter(|issue| issue.pull_request.is_none())
        .collect();

    println!(
        "TANTRIC BOARD — {} ({} open issues)",
        board.repo(),
        cards.len()
    );

    let mut by_lane: BTreeMap<&str, Vec<&GhIssue>> = BTreeMap::new();
    for issue in &cards {
        if let Some(label) = issue.lane_label() {
            by_lane.entry(label).or_default().push(issue);
        }
    }

    for lane in Lane::ALL {
        let label = lane.label();
        println!("\n[{label}]");
        match by_lane.get(label) {
            Some(lane_issues) if !lane_issues.is_empty() => {
                for issue in lane_issues {
                    println!("  #{:<4} {}", issue.number, issue.title);
                }
            }
            _ => println!("  (empty)"),
        }
    }

    let scratch: Vec<&GhIssue> = cards
        .iter()
        .copied()
        .filter(|issue| issue.lane_label().is_none())
        .filter(|issue| issue.title.starts_with("todo: ") || issue.title.starts_with("daily: "))
        .collect();
    if !scratch.is_empty() {
        println!("\n[todo/daily scratch]");
        for issue in scratch {
            println!("  #{:<4} {}", issue.number, issue.title);
        }
    }

    Ok(())
}

/// `todo list` — your todo items, from the `todo: <login>` issue.
async fn cmd_todo_list() -> anyhow::Result<()> {
    let collab = current_collaborator()?;
    let board = Board::require_token()?;
    let prefix = todo_title(collab.login);
    let issues = board.list_issues().await?;
    let Some(issue) = board::find_by_title_prefix(&issues, &prefix) else {
        println!(
            "no todo issue yet for {} — `tantra-agent todo add <text>` creates it",
            collab.login
        );
        return Ok(());
    };

    println!("todo for {} (#{})", collab.login, issue.number);
    if let Some(body) = issue.body.as_deref().filter(|body| !body.trim().is_empty()) {
        println!("  - {body}");
    }
    for comment in board.list_comments(issue.number).await? {
        if !comment.body.trim().is_empty() {
            println!("  - {body}", body = comment.body.trim());
        }
    }
    Ok(())
}

/// `todo add <text>` — append to (or create) the `todo: <login>` issue.
async fn cmd_todo_add(text: &str) -> anyhow::Result<()> {
    ensure_text("todo", text)?;
    let collab = current_collaborator()?;
    let board = Board::require_token()?;
    let title = todo_title(collab.login);
    let issues = board.list_issues().await?;

    if let Some(issue) = board::find_by_title_prefix(&issues, &title) {
        board.add_comment(issue.number, text).await?;
        println!("appended to #{number} ({title})", number = issue.number);
    } else {
        let issue = board.create_issue(&title, text, &[]).await?;
        println!("created #{number} ({title})", number = issue.number);
    }
    Ok(())
}

/// `summary today <text>` — append to (or create) the `daily: <login>` issue.
async fn cmd_summary_today(text: &str) -> anyhow::Result<()> {
    ensure_text("summary", text)?;
    let collab = current_collaborator()?;
    let board = Board::require_token()?;
    let title = daily_title(collab.login);
    let issues = board.list_issues().await?;

    if let Some(issue) = board::find_by_title_prefix(&issues, &title) {
        board.add_comment(issue.number, text).await?;
        println!("appended to #{number} ({title})", number = issue.number);
    } else {
        let issue = board.create_issue(&title, text, &[]).await?;
        println!("created #{number} ({title})", number = issue.number);
    }
    Ok(())
}

/// `whoami` — which collaborator token is active (from env).
fn cmd_whoami() -> anyhow::Result<()> {
    match board::collaborator_from_env() {
        Some(collab) => {
            println!(
                "{login} (via {env_var})",
                login = collab.login,
                env_var = collab.env_var
            );
            Ok(())
        }
        None => Err(anyhow!(
            "no tantric token set — export one of TANTRIC_TOKEN_PETER / \
             TANTRIC_TOKEN_8BIT / TANTRIC_TOKEN_SG"
        )),
    }
}

fn ensure_text(kind: &str, text: &str) -> anyhow::Result<()> {
    if text.trim().is_empty() {
        Err(anyhow!("{kind} text is empty — pass the text to append"))
    } else {
        Ok(())
    }
}
