use anyhow::Context;
use clap::Parser;
use entheai_config::Config;
use entheai_providers::ChatMessage;

// macOS: mimalloc handles the concurrent tokio / multi-agent allocation load
// better than the system allocator. Keep this block across future main.rs rewrites.
#[cfg(target_os = "macos")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// entheai — hybrid coding agent (v0.1)
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The prompt to send. Omit to launch the interactive TUI.
    prompt: Option<String>,
    /// Path to config TOML (default: ./entheai.toml).
    #[arg(long, default_value = "entheai.toml")]
    config: String,
    /// Override model as "<provider>/<model>".
    #[arg(long)]
    model: Option<String>,
    /// Auto-approve all tool calls (skip the permission prompt).
    #[arg(long)]
    yolo: bool,
    /// Disable the companion window for this session.
    #[arg(long)]
    no_companion: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env (DEEPSEEK_API_KEY / HUGGINGFACE_API_KEY / OPENROUTER_API_KEY / ...) into the
    // process env before anything reads keys. A missing .env is fine (ignored).
    dotenvy::dotenv().ok();

    // Crash/error reporting to Sentry (cloud). DSNs are client-embeddable by design;
    // override or disable via the SENTRY_DSN env var. The guard flushes events on drop.
    let dsn = std::env::var("SENTRY_DSN").unwrap_or_else(|_| {
        "https://ea8a1a1d46d9c33b709aae544ff24a79@o4511756214075392.ingest.de.sentry.io/4511756233474128".to_string()
    });
    let _sentry = sentry::init((
        dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
            // no PII capture (CLI has none meaningful; keeps crash reports minimal)
            send_default_pii: false,
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
    let agent = entheai_router::build_agent(&model_id, &cfg)?;

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

    // Companion: spawn the session beacon window if enabled.
    let session_id = uuid::Uuid::new_v4().to_string();
    let _companion = if cfg.companion.enabled && !cli.no_companion {
        let mut args = vec![
            "--session-id".to_string(),
            session_id.clone(),
            "--host".to_string(),
            hostname(),
            "--port".to_string(),
            "9876".to_string(),
            "--cwd".to_string(),
            root.display().to_string(),
        ];
        if !cfg.companion.always_on_top {
            args.push("--no-always-on-top".to_string());
        }
        spawn_companion(&args)
    } else {
        None
    };

    match cli.prompt {
        // One-shot: run the prompt, print the answer, exit.
        Some(prompt) => {
            let mut prompter = entheai_permission::StdinPrompter;
            let messages = vec![ChatMessage::user(prompt)];
            let answer = agent
                .run_task(messages, &registry, &policy, &mut prompter, None)
                .await?;
            println!("{answer}");
        }
        // No prompt: launch the interactive TUI.
        None => {
            entheai_tui::run(agent, registry, policy, model_id.clone()).await?;
        }
    }
    Ok(())
}

/// Resolve the best hostname for the companion QR: Tailscale MagicDNS if
/// available, otherwise the local hostname.
fn hostname() -> String {
    // Try Tailscale first.
    if let Ok(out) = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
    {
        if out.status.success() {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(name) = val["Self"]["DNSName"].as_str() {
                    return name.trim_end_matches('.').to_string();
                }
            }
        }
    }
    // Fall back to local hostname.
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| format!("{}.local", s.trim()))
        .unwrap_or_else(|| "localhost".to_string())
}

/// Try to spawn the companion binary. Returns a handle that kills the child
/// on drop, or `None` if the binary couldn't be found.
fn spawn_companion(args: &[String]) -> Option<CompanionGuard> {
    let (bin, bin_args) = find_companion_binary();
    let child = std::process::Command::new(&bin)
        .args(&bin_args)
        .args(args)
        .spawn()
        .ok()?;
    Some(CompanionGuard { child })
}

/// Search for the companion binary next to the current executable, then fall
/// back to spawning via `cargo run` during development.
fn find_companion_binary() -> (String, Vec<String>) {
    let bin_name = "entheai-companion";

    // Check next to the current executable (release builds).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(bin_name);
            if candidate.exists() {
                return (candidate.display().to_string(), vec![]);
            }
        }
    }

    // Fall back: try PATH (installed or `cargo install`).
    (bin_name.to_string(), vec![])
}

/// Kills the companion child process on drop.
struct CompanionGuard {
    child: std::process::Child,
}

impl Drop for CompanionGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
