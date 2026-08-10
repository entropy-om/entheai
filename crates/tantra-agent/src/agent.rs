//! The minimal adk-rust agent: a `LlmAgent` with the three tantric tools wired,
//! backed by an OpenAI-compatible client to the free `coder.vaked.dev` node
//! (the same endpoint entheai's built-in defaults use). A collaborator can say
//! "move card 3 to burning" and the model resolves it to a `tantra_move` call.

use std::sync::Arc;

use adk_rust::futures::StreamExt;
use adk_rust::model::openai::{OpenAIClient, OpenAIConfig};
use adk_rust::{Content, EventStream};

use crate::board::Board;
use crate::tools::{TantraAdd, TantraList, TantraMove};

/// Default OpenAI-compatible endpoint: the free, keyless vaked inference node.
const MODEL_BASE_URL: &str = "https://coder.vaked.dev/v1";
/// Default model on that node.
const MODEL_NAME: &str = "qwen3-coder:30b";

/// The agent's system instruction.
fn agent_instruction() -> String {
    let mut instr = String::from(
        "You are the tantra-agent, the board-control agent for the tantric board at \
         https://mlxquantlovefrom.com (GitHub issues in \
         peterlodri-sec/mlxquantlovefrom.com).\n\
         \n\
         Use tantra_list to read the board (all lanes and their cards).\n\
         Use tantra_add to create a card (title + lane).\n\
         Use tantra_move to move a card between lanes (number + lane).\n\
         Lanes: backlog, burning, tantra, done.\n\
         Only the three collaborators (peterlodri-sec, 8bit-wraith, \
         standardgalactic) can write — each with their own TANTRIC_TOKEN_* env \
         var. If tantra_add/tantra_move report a missing token, say plainly \
         that the caller needs to set one.\n\
         Be terse: report card numbers and lanes, not commentary.",
    );
    if let Some(collab) = crate::board::collaborator_from_env() {
        instr.push_str(&format!(
            "\n\nYou are acting as {login} (token from {env_var}).",
            login = collab.login,
            env_var = collab.env_var
        ));
    }
    instr
}

/// Run one agent turn: the model sees the tools and resolves `prompt` into
/// board operations. Returns the model's final text answer.
///
/// Endpoint / model overridable via `TANTRIC_MODEL_URL` / `TANTRIC_MODEL_NAME`
/// (and an optional `TANTRIC_MODEL_API_KEY` for a paid tier).
pub async fn run_agent(prompt: &str) -> anyhow::Result<String> {
    let api_key =
        std::env::var("TANTRIC_MODEL_API_KEY").unwrap_or_else(|_| "not-needed".to_string());
    let base_url =
        std::env::var("TANTRIC_MODEL_URL").unwrap_or_else(|_| MODEL_BASE_URL.to_string());
    let model_name = std::env::var("TANTRIC_MODEL_NAME").unwrap_or_else(|_| MODEL_NAME.to_string());

    let config = OpenAIConfig::compatible(api_key, base_url, model_name);
    let model = OpenAIClient::new(config)?;

    let board = Board::from_env()?;
    let agent = adk_rust::agent::LlmAgentBuilder::new("tantra-agent")
        .description("Board-control agent for the tantric board (mlxquantlovefrom.com)")
        .instruction(agent_instruction())
        .model(Arc::new(model))
        .tool(Arc::new(TantraList::new(board.clone())))
        .tool(Arc::new(TantraAdd::new(board.clone())))
        .tool(Arc::new(TantraMove::new(board.clone())))
        .build()?;

    let session_service: Arc<dyn adk_rust::session::SessionService> =
        Arc::new(adk_rust::session::InMemorySessionService::new());
    session_service
        .create(adk_rust::session::CreateRequest {
            app_name: "tantra-agent".into(),
            user_id: "user".into(),
            session_id: Some("tantra-session".into()),
            state: Default::default(),
        })
        .await?;

    let runner = adk_rust::runner::Runner::builder()
        .app_name("tantra-agent")
        .agent(Arc::new(agent))
        .session_service(session_service)
        .build()?;

    let content = Content::new("user").with_text(prompt.to_string());
    let mut stream: EventStream = runner.run_str("user", "tantra-session", content).await?;

    let mut answer = String::new();
    while let Some(event) = stream.next().await {
        let event = event?;
        if let Some(content) = &event.llm_response.content {
            for part in &content.parts {
                if let Some(text) = part.text() {
                    answer.push_str(text);
                }
            }
        }
    }
    Ok(answer)
}
