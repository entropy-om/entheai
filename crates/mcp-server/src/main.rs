//! `entheai-mcp` — the opencode↔entheai bridge, Phase 2: a library-backed MCP
//! stdio server exposing entheai's engine capabilities as typed `entheai_*`
//! tools. Nothing here shells out + parses text: every tool calls the engine's
//! crates as a library, so the full `MergeSeal`, fleet presence, typed errors,
//! memory and skills all surface natively.
//!
//! Protocol: newline-delimited JSON-RPC 2.0 over stdio (the same wire entheai's
//! own `crates/mcp` client speaks), so `command: ["entheai-mcp"]` works from
//! opencode, Claude, and entheai's own `[mcp.*]` config alike.

mod board_tools;
mod dispatch_tool;
mod engine;
mod fanout_tool;
mod fleet_tool;
mod memory_tools;
mod protocol;
mod run_tool;
mod skills_tools;
mod tools;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // The server process's own .env (when launched from a repo root). Per-call
    // `.env` files are additionally loaded by each tool against its `cwd`.
    let _ = dotenvy::dotenv();

    let server_cwd = std::env::current_dir()?.canonicalize()?;
    let tools = tools::all();
    log::info!(
        "entheai-mcp v{} serving {} tools from {:?}",
        env!("CARGO_PKG_VERSION"),
        tools.len(),
        server_cwd
    );
    protocol::serve(&tools, server_cwd).await
}
