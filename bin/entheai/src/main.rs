use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use entheai_companion::state::StateChange;
use entheai_config::Config;
use entheai_providers::ChatMessage;
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
    /// Decompose the task and fan out parallel sub-agents, then synthesize.
    #[arg(long)]
    fanout: bool,
    /// Disable the companion window for this session.
    #[arg(long)]
    no_companion: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _sentry = init_telemetry();
    let cli = Cli::parse();

    let cfg_text = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("reading config {}", cli.config))?;
    let cfg = Config::from_toml_str(&cfg_text)?;
    let root = std::env::current_dir()?.canonicalize()?;

    // Tool registry (built-ins + skills + MCP servers) + the skills system prompt.
    // `_mcp_guards` keeps the spawned MCP child processes alive for the session.
    let (registry, system_prompt, _mcp_guards) = build_tools(&root, &cfg).await?;

    let model_id = cli
        .model
        .clone()
        .or(cfg.default_model.clone())
        .context("no model: pass --model or set default_model in config")?;
    let agent = entheai_router::build_agent(&model_id, &cfg)?;
    let policy = entheai_permission::Policy::new(cli.yolo, vec![]);

    // Shared memory store (open before any agent run so the DB + parent dir exist
    // even when the model call fails) + a session id for scoping.
    let shared_memory = build_memory(&cfg)?;
    let session_id = uuid::Uuid::new_v4().to_string();

    let companion = setup_companion(&cfg, &root, cli.no_companion)?;
    if let Some(ref c) = companion {
        let _ = c.state_tx.send(StateChange::working());
    }

    match cli.prompt {
        Some(prompt) => {
            if cli.fanout {
                let pool = entheai_orchestrator::WorkerPool::new(cfg.router.max_parallel.max(1));
                let answer =
                    entheai_orchestrator::run_fanout(&cfg, &root, &prompt, None, pool).await?;
                println!("{answer}");
            } else {
                let mut prompter = entheai_permission::StdinPrompter;
                let mut messages = Vec::new();
                if let Some(sp) = &system_prompt {
                    messages.push(ChatMessage::system(sp.clone()));
                }
                messages.push(ChatMessage::user(prompt));
                let runtime = shared_memory.clone().map(|m| {
                    entheai_memory::MemoryRuntime::new(m, memory_runtime_config(&cfg.memory))
                });
                let scope = entheai_memory::MemoryScope {
                    session_id: session_id.clone(),
                    task_id: "oneshot".to_string(),
                    cwd: root.clone(),
                    role: None,
                };
                let answer = agent
                    .run_task_with_memory(
                        messages,
                        &registry,
                        &policy,
                        &mut prompter,
                        None,
                        runtime.as_ref(),
                        scope,
                    )
                    .await?;
                println!("{answer}");
            }
        }
        None => {
            let companion_tx = companion.as_ref().map(|c| c.state_tx.clone());
            entheai_tui::run(
                agent,
                registry,
                policy,
                model_id.clone(),
                cfg,
                root.clone(),
                cli.fanout,
                system_prompt,
                companion_tx,
            )
            .await?;
        }
    }
    Ok(())
}

/// Load `.env` and initialize Sentry crash reporting (PII disabled). The
/// returned guard flushes events on drop, so `main` must hold it.
fn init_telemetry() -> sentry::ClientInitGuard {
    dotenvy::dotenv().ok();
    let dsn = std::env::var("SENTRY_DSN").unwrap_or_else(|_| {
        "https://ea8a1a1d46d9c33b709aae544ff24a79@o4511756214075392.ingest.de.sentry.io/4511756233474128".to_string()
    });
    sentry::init((
        dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
            send_default_pii: false,
            ..Default::default()
        },
    ))
}

/// Expand a leading `~` to the user's home directory.
fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Map the config's `[memory]` block to the runtime config.
fn memory_runtime_config(m: &entheai_config::MemoryConfig) -> entheai_memory::MemoryRuntimeConfig {
    entheai_memory::MemoryRuntimeConfig {
        enabled: m.enabled,
        strict: m.strict,
        retrieve_codebase: m.retrieve_codebase,
        retrieve_learnings: m.retrieve_learnings,
        retrieve_trajectories: m.retrieve_trajectories,
        max_context_chars: m.max_context_chars,
        tool_spill_chars: m.tool_spill_chars,
        evidence_tools: if m.evidence_tools.is_empty() {
            vec!["run_shell".into(), "search".into()]
        } else {
            m.evidence_tools.clone()
        },
    }
}

/// Build the shared memory store from config: an optional embedder (only when
/// `embed_provider` is configured — keeps on-by-default offline-safe) plus the
/// recall params. Returns `None` when memory is disabled.
fn build_memory(cfg: &Config) -> anyhow::Result<Option<entheai_memory::SharedMemory>> {
    if !cfg.memory.enabled {
        return Ok(None);
    }
    let embedder = cfg.memory.embed_provider.as_ref().and_then(|p| {
        cfg.providers.get(p).map(|pc| {
            entheai_memory::Embedder::new(pc.base_url.clone(), cfg.memory.embed_model.clone())
        })
    });
    let path = expand_home(&cfg.memory.path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut store = entheai_memory::SqliteStore::open(&path, embedder)?;
    store.set_recall_params(entheai_memory::RecallParams {
        w_recency: cfg.memory.w_recency,
        w_conf: cfg.memory.w_conf,
        half_life_days: cfg.memory.half_life_days,
        rrf_k: cfg.memory.rrf_k,
        overfetch: cfg.memory.recall_overfetch,
    });
    Ok(Some(std::sync::Arc::new(store)))
}

/// Build the tool registry (built-in fs/shell/search tools + discovered skills +
/// configured MCP servers) and the skills system prompt. Returns the registry,
/// the system prompt (if any skills were found), and the MCP child-process
/// guards (which the caller must keep alive for the session).
async fn build_tools(
    root: &Path,
    cfg: &Config,
) -> anyhow::Result<(
    entheai_tools::ToolRegistry,
    Option<String>,
    Vec<entheai_mcp::ChildGuard>,
)> {
    let mut registry = entheai_tools::ToolRegistry::new();
    registry.register(Box::new(entheai_tools::fs::ReadFile::new(
        root.to_path_buf(),
    )));
    registry.register(Box::new(entheai_tools::fs::WriteFile::new(
        root.to_path_buf(),
    )));
    registry.register(Box::new(entheai_tools::fs::EditFile::new(
        root.to_path_buf(),
    )));
    registry.register(Box::new(
        entheai_tools::shell::RunShell::new(root.to_path_buf())
            .with_limits(cfg.tools.shell_timeout_secs, cfg.tools.shell_output_cap),
    ));
    registry.register(Box::new(
        entheai_tools::search::Search::new(root.to_path_buf())
            .with_max_results(cfg.tools.search_max_results),
    ));
    registry.register(Box::new(entheai_tools::todo::TodoTool));

    // Skills: discover, advertise via a system prompt, expose the `skill` tool.
    let skill_dirs: Vec<PathBuf> = cfg.skills.dirs.iter().map(|d| root.join(d)).collect();
    let skills = std::sync::Arc::new(entheai_skills::SkillRegistry::discover(&skill_dirs));
    let system_prompt = if skills.is_empty() {
        None
    } else {
        registry.register(Box::new(entheai_skills::SkillTool::new(
            std::sync::Arc::clone(&skills),
        )));
        eprintln!(
            "skills: loaded {} ({})",
            skills.list().len(),
            skills
                .list()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        Some(skills.advertisement())
    };

    let todo_hint = "Use the `todo` tool to publish and keep your plan up to date — set items to in_progress/done as you work.";
    let system_prompt = Some(match system_prompt {
        Some(skills_ad) => format!("{skills_ad}\n\n{todo_hint}"),
        None => todo_hint.to_string(),
    });

    // MCP servers: spawn each configured server, register its tools. A server
    // that fails or hangs is skipped with a warning (never blocks startup).
    let mut guards = Vec::new();
    for (name, mcp_cfg) in &cfg.mcp {
        let load = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let (client, guard) =
                entheai_mcp::McpClient::spawn(&mcp_cfg.command, &mcp_cfg.args, name).await?;
            let tools = entheai_mcp::load_tools(client).await?;
            Ok::<_, entheai_mcp::McpError>((guard, tools))
        })
        .await;
        match load {
            Ok(Ok((guard, tools))) => {
                let n = tools.len();
                for tool in tools {
                    registry.register(Box::new(tool));
                }
                eprintln!("mcp: '{name}' connected ({n} tool(s))");
                guards.push(guard);
            }
            Ok(Err(e)) => eprintln!("mcp: '{name}' failed: {e}"),
            Err(_) => eprintln!("mcp: '{name}' timed out after 10s — skipping"),
        }
    }

    Ok((registry, system_prompt, guards))
}

/// Spawn the companion beacon window (if enabled): bind a session Unix socket,
/// forward `StateChange` events to it over a background task, and launch the
/// companion child process. Returns a handle that kills the child + removes the
/// socket on drop. `None` when the companion is disabled.
fn setup_companion(
    cfg: &Config,
    root: &Path,
    no_companion: bool,
) -> anyhow::Result<Option<CompanionHandle>> {
    if !cfg.companion.enabled || no_companion {
        return Ok(None);
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let socket_path = std::env::temp_dir().join(format!("entheai-{session_id}.sock"));
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    let (state_tx, mut state_rx) = tokio::sync::mpsc::unbounded_channel::<StateChange>();

    // Accept the companion connection and stream newline-delimited events to it.
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            while let Some(change) = state_rx.recv().await {
                let json = serde_json::to_string(&change).unwrap_or_default();
                if stream.write_all(json.as_bytes()).await.is_err()
                    || stream.write_all(b"\n").await.is_err()
                {
                    break;
                }
            }
        }
    });

    let mut args = vec![
        "--session-id".to_string(),
        session_id,
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

    Ok(Some(CompanionHandle {
        child,
        state_tx,
        socket_path,
    }))
}

/// Resolve a hostname for the companion QR: Tailscale MagicDNS if available,
/// else the local hostname.
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

/// Locate the `entheai-companion` binary next to the current executable, else
/// fall back to the name on `PATH`.
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

/// Owns the companion child process + its session socket; cleans both up on drop.
struct CompanionHandle {
    child: Option<std::process::Child>,
    state_tx: tokio::sync::mpsc::UnboundedSender<StateChange>,
    socket_path: PathBuf,
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
