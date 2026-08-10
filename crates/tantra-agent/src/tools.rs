//! The adk-rust `Tool` impls that give the tantra-agent LLM board control:
//! `tantra_list` (read), `tantra_add` (create a card), `tantra_move` (move a
//! card between lanes). The CLI is the primary surface; these are the thin
//! agent wrapper over the same `Board` client.

use std::sync::Arc;

use adk_rust::serde_json::{json, Value};
use adk_rust::{async_trait, Result as AdkResult, Tool as AdkTool, ToolContext};

use crate::board::{Board, Lane};

/// Read the board: every lane and its cards (number, title, lane).
#[derive(Debug, Clone)]
pub struct TantraList {
    board: Board,
}

impl TantraList {
    /// A list tool over the given board client.
    pub fn new(board: Board) -> Self {
        Self { board }
    }

    /// A list tool over a board resolved from the environment.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::new(Board::from_env()?))
    }
}

#[async_trait]
impl AdkTool for TantraList {
    fn name(&self) -> &str {
        "tantra_list"
    }

    fn description(&self) -> &str {
        "List the tantric board (mlxquantlovefrom.com): all lanes \
         (backlog/burning/tantra/done) with their cards. Each card has a \
         number, a title, and a lane."
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(json!({ "type": "object", "properties": {} }))
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, _args: Value) -> AdkResult<Value> {
        match self.board.list_issues().await {
            Ok(issues) => {
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
            Err(e) => Ok(json!({ "error": e.to_string() })),
        }
    }
}

/// Create a card (an issue with the lane label). Needs a collaborator token.
#[derive(Debug, Clone)]
pub struct TantraAdd {
    board: Board,
}

impl TantraAdd {
    /// An add tool over the given board client.
    pub fn new(board: Board) -> Self {
        Self { board }
    }

    /// An add tool over a board resolved from the environment.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::new(Board::require_token()?))
    }
}

#[async_trait]
impl AdkTool for TantraAdd {
    fn name(&self) -> &str {
        "tantra_add"
    }

    fn description(&self) -> &str {
        "Create a new card on the tantric board (mlxquantlovefrom.com): a \
         GitHub issue with the lane label. Arguments: title (string), lane \
         (backlog|burning|tantra|done, default tantra). Requires a \
         TANTRIC_TOKEN_* env var (only the three board collaborators can \
         write)."
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "The card title." },
                "lane": {
                    "type": "string",
                    "enum": ["backlog", "burning", "tantra", "done"],
                    "description": "Lane to put the card in (default tantra)."
                }
            },
            "required": ["title"]
        }))
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, args: Value) -> AdkResult<Value> {
        let Some(title) = args.get("title").and_then(Value::as_str) else {
            return Ok(json!({ "error": "missing required argument \"title\"" }));
        };
        let lane = args
            .get("lane")
            .and_then(Value::as_str)
            .and_then(Lane::parse)
            .unwrap_or(Lane::Tantra);
        match self.board.create_issue(title, "", &[lane.label()]).await {
            Ok(issue) => Ok(json!({
                "number": issue.number,
                "title": issue.title,
                "lane": lane.label(),
            })),
            Err(e) => Ok(json!({ "error": e.to_string() })),
        }
    }
}

/// Move a card between lanes (update its labels). Needs a collaborator token.
#[derive(Debug, Clone)]
pub struct TantraMove {
    board: Board,
}

impl TantraMove {
    /// A move tool over the given board client.
    pub fn new(board: Board) -> Self {
        Self { board }
    }

    /// A move tool over a board resolved from the environment.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::new(Board::require_token()?))
    }
}

#[async_trait]
impl AdkTool for TantraMove {
    fn name(&self) -> &str {
        "tantra_move"
    }

    fn description(&self) -> &str {
        "Move a card on the tantric board (mlxquantlovefrom.com) to another \
         lane: PATCH the issue's labels. Arguments: number (integer, the card \
         number), lane (backlog|burning|tantra|done). Requires a \
         TANTRIC_TOKEN_* env var (only the three board collaborators can \
         write)."
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "number": { "type": "integer", "description": "The card number." },
                "lane": {
                    "type": "string",
                    "enum": ["backlog", "burning", "tantra", "done"],
                    "description": "The lane to move the card to."
                }
            },
            "required": ["number", "lane"]
        }))
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, args: Value) -> AdkResult<Value> {
        let Some(number) = args.get("number").and_then(Value::as_u64) else {
            return Ok(json!({ "error": "missing required argument \"number\"" }));
        };
        let Some(lane) = args
            .get("lane")
            .and_then(Value::as_str)
            .and_then(Lane::parse)
        else {
            return Ok(
                json!({ "error": "missing or unknown argument \"lane\" (backlog|burning|tantra|done)" }),
            );
        };
        match self.board.update_labels(number, &[lane.label()]).await {
            Ok(issue) => Ok(json!({
                "number": issue.number,
                "title": issue.title,
                "lane": lane.label(),
            })),
            Err(e) => Ok(json!({ "error": e.to_string() })),
        }
    }
}
