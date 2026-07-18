use std::io::Write;

use anyhow::Context;
use clap::Parser;
use entheai_config::Config;
use entheai_core::{Agent, TokenSink};
use entheai_providers::{ChatMessage, OpenAiCompatProvider};

/// entheai — hybrid coding agent (v0.1 skeleton)
#[derive(Parser)]
struct Cli {
    /// The prompt to send.
    prompt: String,
    /// Path to config TOML (default: ./entheai.toml).
    #[arg(long, default_value = "entheai.toml")]
    config: String,
    /// Override model as "<provider>/<model>".
    #[arg(long)]
    model: Option<String>,
}

struct StdoutSink;
impl TokenSink for StdoutSink {
    fn emit(&mut self, token: &str) {
        print!("{token}");
        let _ = std::io::stdout().flush();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let cfg_text = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("reading config {}", cli.config))?;
    let cfg = Config::from_toml_str(&cfg_text)?;

    let model_id = cli
        .model
        .or(cfg.default_model.clone())
        .context("no model: pass --model or set default_model in config")?;
    let (provider_name, model) = model_id
        .split_once('/')
        .context("model must be '<provider>/<model>'")?;

    let pcfg = cfg
        .providers
        .get(provider_name)
        .with_context(|| format!("unknown provider '{provider_name}'"))?;
    let api_key = pcfg
        .api_key_env
        .as_ref()
        .and_then(|e| std::env::var(e).ok());

    let provider = OpenAiCompatProvider::new(pcfg.base_url.clone(), api_key);
    let agent = Agent::new(provider, model.to_string());

    let mut sink = StdoutSink;
    let messages = vec![ChatMessage {
        role: "user".into(),
        content: cli.prompt,
    }];
    agent.run_turn(messages, &mut sink).await?;
    println!();
    Ok(())
}
