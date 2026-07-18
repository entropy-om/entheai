use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use entheai_companion::state::StateChange;
use entheai_config::Config;
use entheai_core::Agent;
use entheai_providers::{ChatMessage, OpenAiCompatProvider};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;

#[cfg(target_os = "macos")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(version)]
struct Cli {
    prompt: Option<String>,
    #[arg(long, default_value = "entheai.toml")]
    config: String,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    yolo: bool,
    #[arg(long)]
    no_companion: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let dsn = std::env::var("SENTRY_DSN").unwrap_or_else(|_| {
        "https://ea8a1a1d46d9c33b709aae544ff24a79@o4511756214075392.ingest.de.sentry.io/4511756233474128".to_string()
    });
    let _sentry = sentry::init((
        dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
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

    let session_id = uuid::Uuid::new_v4().to_string();
    let companion = if cfg.companion.enabled && !cli.no_companion {
        let socket_path = std::env::temp_dir().join(format!("entheai-{}.sock", session_id));
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path)?;
        let (state_tx, mut state_rx) = tokio::sync::mpsc::unbounded_channel::<StateChange>();

        // Spawn a task that accepts the companion connection and forwards events.
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                while let Some(change) = state_rx.recv().await {
                    let json = serde_json::to_string(&change).unwrap();
                    if stream.write_all(json.as_bytes()).await.is_err() {
                        break;
                    }
                    if stream.write_all(b"\n").await.is_err() {
                        break;
                    }
                }
            }
        });

        let mut args = vec![
            "--session-id".to_string(),
            session_id.clone(),
            "--host".to_string(),
            hostname(),
            "--port".to_string(),
            "9876".to_string(),
            "--cwd".to_string(),
            root.display().to_string(),
            "--socket".to_string(),
            socket_path.display().to_string(),
        ];
        if !cfg.companion.always_on_top {
            args.push("--no-always-on-top".to_string());
        }

        let (bin, _) = find_companion_binary();
        let child = std::process::Command::new(&bin).args(&args).spawn().ok();

        Some(CompanionHandle {
            child,
            state_tx,
            socket_path,
        })
    } else {
        None
    };

    // Send initial state.
    if let Some(ref c) = companion {
        let _ = c.state_tx.send(StateChange::working());
    }

    match cli.prompt {
        Some(prompt) => {
            let mut prompter = entheai_permission::StdinPrompter;
            let messages = vec![ChatMessage::user(prompt)];
            let answer = agent
                .run_task(messages, &registry, &policy, &mut prompter, None)
                .await?;
            println!("{answer}");
        }
        None => {
            entheai_tui::run(agent, registry, policy, model_id.clone()).await?;
        }
    }
    Ok(())
}

fn hostname() -> String {
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
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| format!("{}.local", s.trim()))
        .unwrap_or_else(|| "localhost".to_string())
}

fn find_companion_binary() -> (String, Vec<String>) {
    let bin_name = "entheai-companion";
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(bin_name);
            if candidate.exists() {
                return (candidate.display().to_string(), vec![]);
            }
        }
    }
    (bin_name.to_string(), vec![])
}

struct CompanionHandle {
    child: Option<std::process::Child>,
    state_tx: tokio::sync::mpsc::UnboundedSender<StateChange>,
    socket_path: PathBuf,
}

impl CompanionHandle {
    #[allow(dead_code)]
    fn send_state(&self, change: StateChange) {
        let _ = self.state_tx.send(change);
    }
}

impl Drop for CompanionHandle {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
