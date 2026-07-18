use anyhow::Context;
use clap::Parser;
use entheai_config::Config;
use entheai_core::Agent;
use entheai_providers::{ChatMessage, OpenAiCompatProvider};

// macOS: mimalloc handles the concurrent tokio / multi-agent allocation load
// better than the system allocator. Keep this block across future main.rs rewrites.
#[cfg(target_os = "macos")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// entheai — hybrid coding agent (v0.1)
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
    /// Auto-approve all tool calls (skip the permission prompt).
    #[arg(long)]
    yolo: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Crash/error reporting to Sentry (cloud). DSNs are client-embeddable by design;
    // override or disable via the SENTRY_DSN env var. The guard flushes events on drop.
    let dsn = std::env::var("SENTRY_DSN").unwrap_or_else(|_| {
        "https://ea8a1a1d46d9c33b709aae544ff24a79@o4511756214075392.ingest.de.sentry.io/4511756233474128".to_string()
    });
    let _sentry = sentry::init((
        dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
            send_default_pii: true,
            ..Default::default()
        },
    ));

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

    // Built-in tools, rooted at the canonicalized working directory.
    let root = std::env::current_dir()?.canonicalize()?;
    let mut registry = entheai_tools::ToolRegistry::new();
    registry.register(Box::new(entheai_tools::fs::ReadFile::new(root.clone())));
    registry.register(Box::new(entheai_tools::fs::WriteFile::new(root.clone())));
    registry.register(Box::new(entheai_tools::shell::RunShell::new(root.clone())));
    registry.register(Box::new(entheai_tools::search::Search::new(root.clone())));

    let policy = entheai_permission::Policy {
        yolo: cli.yolo,
        allowlist: vec![],
    };
    let mut prompter = entheai_permission::StdinPrompter;

    let messages = vec![ChatMessage::user(cli.prompt)];
    let answer = agent
        .run_task(messages, &registry, &policy, &mut prompter)
        .await?;
    println!("{answer}");
    Ok(())
}
