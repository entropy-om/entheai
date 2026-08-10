//! `entheai_board_*` tools — read/add the tantric board through the
//! `tantra-agent` crate. The board is GitHub-issues-backed
//! (`peterlodri-sec/mlxquantlovefrom.com`); only the three collaborators
//! (peterlodri-sec, 8bit-wraith, standardgalactic) have write, each with their
//! own `TANTRIC_TOKEN_*` env var.

use std::path::PathBuf;

use serde_json::{json, Value};
use tantra_agent::board::{Board, Lane};

/// `entheai_board_list` — read all lanes + cards (public read; token optional).
pub async fn entheai_board_list(_args: Value, _cwd: PathBuf) -> anyhow::Result<Value> {
    let board = Board::from_env()?;
    let issues = board.list_issues().await?;
    let cards: Vec<Value> = issues
        .iter()
        .filter(|issue| issue.pull_request.is_none())
        .map(|issue| {
            json!({
                "number": issue.number,
                "title": issue.title,
                "lane": issue.lane_label().unwrap_or("no-lane"),
            })
        })
        .collect();
    Ok(json!({ "cards": cards, "count": cards.len() }))
}

/// `entheai_board_add` — create a card (issue with the lane label). Write
/// access requires a `TANTRIC_TOKEN_*` env var.
pub async fn entheai_board_add(args: Value, _cwd: PathBuf) -> anyhow::Result<Value> {
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing required string arg \"title\""))?;
    let lane = match args.get("lane").and_then(Value::as_str) {
        Some(raw) => Lane::parse(raw).ok_or_else(|| {
            anyhow::anyhow!("unknown lane {raw:?} — expected backlog|burning|tantra|done")
        })?,
        None => Lane::Tantra,
    };
    let board = Board::require_token()?;
    let issue = board.create_issue(title, "", &[lane.label()]).await?;
    Ok(json!({
        "number": issue.number,
        "title": issue.title,
        "lane": lane.label(),
        "html_url": issue.html_url,
    }))
}
