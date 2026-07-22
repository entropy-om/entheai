use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use entheai_companion::state::StateChange;
use entheai_config::Config;
use entheai_providers::ChatMessage;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;

mod logging;

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
    /// Open entheai in a dedicated minimalist Ghostty window (the native-app experience).
    #[arg(long)]
    app: bool,
    /// Install the rain-on-glass shader into your own Ghostty config, then exit.
    #[arg(long)]
    doctor: bool,
    /// Inspect memory then exit: `--memory stats`, `--memory list <namespace>`,
    /// `--memory search <namespace> <query...>`.
    #[arg(long = "memory", num_args = 1.., value_name = "ARGS")]
    memory: Vec<String>,
    /// Add a skill from a URL, then exit: `--skills add <url>`. Discovers via
    /// <origin>/.well-known/skills.json, then /llms.txt, then the page.
    #[arg(long = "skills", num_args = 1.., value_name = "ARGS")]
    skills: Option<Vec<String>>,
}

/// The `--config` default; only this filename falls through to the global /
/// built-in configs when absent (an explicit `--config <other>` must exist).
const DEFAULT_CONFIG_PATH: &str = "entheai.toml";

/// Built-in configuration used when no `entheai.toml` is found in the working
/// directory or `~/.config/entheai/`. Keeps the standard OpenAI-compatible
/// providers — keys still come from the environment / `.env` — so `entheai` runs
/// from any directory. Deliberately omits user-specific MCP servers and paths.
const DEFAULT_CONFIG_TOML: &str = r#"default_model = "deepseek/deepseek-chat"

[providers.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

[providers.hf]
base_url = "https://router.huggingface.co/v1"
api_key_env = "HUGGINGFACE_API_KEY"

[providers.zen]
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_API_KEY"

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"

[router]
orchestrator = "deepseek/deepseek-chat"
max_parallel = 4
"#;

/// Load the config, tolerating a missing default file so `entheai` runs from any
/// directory (fixes a hard `reading config entheai.toml: No such file or
/// directory` when launched outside a project). Resolution order:
///   1. `path` in the cwd — a file that *is* present must parse (a broken config
///      is a real error, never silently ignored).
///   2. `~/.config/entheai/entheai.toml` — the per-user global config.
///   3. Built-in defaults ([`DEFAULT_CONFIG_TOML`]).
///
/// An *explicitly* passed `--config <other>` that is missing stays a hard error;
/// only the default filename falls through to the global / built-in configs.
fn load_config(path: &str) -> anyhow::Result<Config> {
    if std::path::Path::new(path).exists() {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {path}"))?;
        return Ok(Config::from_toml_str(&text)?);
    }

    if path != DEFAULT_CONFIG_PATH {
        anyhow::bail!("reading config {path}: No such file or directory (os error 2)");
    }

    let global = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".config")
        .join("entheai")
        .join(DEFAULT_CONFIG_PATH);
    if global.exists() {
        let text = std::fs::read_to_string(&global)
            .with_context(|| format!("reading config {}", global.display()))?;
        eprintln!(
            "entheai: no ./{DEFAULT_CONFIG_PATH} — using {}",
            global.display()
        );
        return Ok(Config::from_toml_str(&text)?);
    }

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    eprintln!(
        "entheai: no {DEFAULT_CONFIG_PATH} in {cwd} or {} — using built-in defaults \
         (provider keys still come from the environment / .env; run from a project \
         with an entheai.toml, or `entheai --config <path>`, for custom settings)",
        global.parent().map(|p| p.display().to_string()).unwrap_or_default(),
    );
    Ok(Config::from_toml_str(DEFAULT_CONFIG_TOML)?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` first so provider keys, MCP URLs, etc. are visible to
    // everything downstream (config parsing, providers, MCP spawn).
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    // Install the log backend before anything can emit. Interactive TUI sessions
    // (no prompt, no `--memory`, no `--app`) log to a file only so the alternate
    // screen is never corrupted; every other mode mirrors to stderr too.
    let interactive = cli.prompt.is_none()
        && cli.memory.is_empty()
        && cli.skills.is_none()
        && !cli.app
        && !cli.doctor;
    logging::init(interactive);

    // `--app` opens a dedicated minimalist Ghostty window running plain `entheai`
    // (no `--app`, so no recursion). Short-circuit before any config-file read so
    // launching the native app never requires an `entheai.toml`.
    if cli.app {
        return entheai_launcher::launch();
    }

    // `--doctor` installs the rain-on-glass shader into the user's own Ghostty
    // config and exits — needs no config file, agent, or companion.
    if cli.doctor {
        return run_doctor_cmd();
    }

    let cfg = load_config(&cli.config)?;
    let _sentry = init_telemetry(cfg.telemetry.sentry_dsn.clone());
    let root = std::env::current_dir()?.canonicalize()?;

    // Memory inspection short-circuits before the tool registry or companion are
    // built — it needs neither, and must exit without running the agent.
    if !cli.memory.is_empty() {
        return run_memory_cmd(&cfg, &cli.memory).await;
    }

    // `--skills add <url>` fetches + installs a skill via layered well-known
    // discovery, then exits before the tool registry or companion are built.
    if let Some(skills_args) = cli.skills.as_ref() {
        return run_skills_cmd(skills_args, &cfg, &root).await;
    }

    // Tool registry (built-ins + skills + MCP servers) + the skills system prompt.
    // `_mcp_guards` keeps the spawned MCP child processes alive for the session.
    let (registry, system_prompt, _mcp_guards) = build_tools(&root, &cfg).await?;

    let model_id = cli
        .model
        .clone()
        .or(cfg.default_model.clone())
        .unwrap_or_else(|| entheai_router::DEFAULT_ORCHESTRATOR.to_string());
    let yolo = cli.yolo || cfg.permission.yolo;
    let agent = entheai_router::build_agent(&model_id, &cfg)?;
    // YOLO lifts the turn cap entirely — a long autonomous run shouldn't be cut
    // off at `[router].max_turns` (default 200).
    let agent = if yolo {
        agent.with_max_turns(usize::MAX)
    } else {
        agent
    };
    let mut policy = entheai_permission::Policy::new(yolo, cfg.permission.allowlist.clone());
    let mode = if yolo {
        entheai_permission::Mode::Yolo
    } else {
        entheai_permission::Mode::parse(&cfg.permission.mode)
    };
    policy.set_mode(mode);
    for (t, p_str) in &cfg.permission.pins {
        let pin = match p_str.trim().to_ascii_lowercase().as_str() {
            "always_allow" | "allow" => entheai_permission::Pin::AlwaysAllow,
            "always_ask" | "ask" => entheai_permission::Pin::AlwaysAsk,
            "never" | "deny" => entheai_permission::Pin::Never,
            _ => {
                log::warn!("unknown pin {p_str:?} for tool {t}; ignoring");
                continue;
            }
        };
        policy.pin(t, pin);
    }

    // Shared memory store (open before any agent run so the DB + parent dir exist
    // even when the model call fails) + a session id for scoping.
    let shared_memory = build_memory(&cfg)?;
    let session_id = uuid::Uuid::new_v4().to_string();

    // Obsidian wiki-sync: session-scoped, fail-safe, stops on drop at end of main.
    let _obsidian = entheai_obsidian::start(
        &obsidian_options(&cfg.obsidian),
        &root,
        std::path::Path::new(&std::env::var("HOME").unwrap_or_default()),
    );

    let companion = setup_companion(&cfg, &root, cli.no_companion)?;
    if let Some(ref c) = companion {
        let _ = c.state_tx.send(StateChange::working());
    }

    match cli.prompt {
        Some(prompt) => {
            if cli.fanout {
                let pool = entheai_orchestrator::WorkerPool::new(cfg.router.max_parallel.max(1));
                // Federation event bus (F1): opt-in + fail-safe. With `[nats]`
                // off or the hub unreachable, `connect` returns None and `tee`
                // hands `None` straight to run_fanout — behavior unchanged.
                let bus = entheai_bus::Bus::connect(
                    &entheai_bus::BusOptions::from_config(&cfg.nats),
                )
                .await;
                let (events, bus_session) =
                    entheai_bus::tee(bus, session_id.clone(), None);
                // Federation dispatch (F2.2): when `[federation]` is on and a
                // worker is serving, coders run on the fleet; otherwise run_fanout
                // runs them locally. Connect failure → None → local (fail-safe).
                let fed_exec: Option<std::sync::Arc<dyn entheai_orchestrator::CoderExecutor>> =
                    if cfg.fanout.executor == "agy" {
                        // Recursive-dev path: each coder runs via the Antigravity CLI
                        // (depth-guarded). agy missing/at-cap → local fallback.
                        Some(entheai_orchestrator::AgyExecutor::new(cfg.fanout.agy_model.clone())
                            as std::sync::Arc<dyn entheai_orchestrator::CoderExecutor>)
                    } else if cfg.federation.enabled {
                        entheai_federation::Federation::connect(
                            &entheai_federation::FedOptions::from_config(&cfg.nats, &cfg.federation),
                        )
                        .await
                        .map(|f| {
                            entheai_federation::FederationExecutor::new(f, root.clone())
                                as std::sync::Arc<dyn entheai_orchestrator::CoderExecutor>
                        })
                    } else {
                        None
                    };
                let answer =
                    entheai_orchestrator::run_fanout(&cfg, &root, &prompt, events, pool, fed_exec)
                        .await?;
                // Drain + flush the tee before teardown so the final events
                // (e.g. `done`) actually reach subscribers. No-op when NATS off.
                bus_session.finish().await;
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
                let pp = build_prompt_processor(&cfg)?;
                if let Some(p) = &pp {
                    let retention = cfg
                        .memory
                        .prompt_processing
                        .as_ref()
                        .map(|c| c.raw_retention_days)
                        .unwrap_or(90);
                    p.prune(retention).await;
                }
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
                        pp.as_ref(),
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

/// Initialize Sentry crash reporting (PII disabled). Resolves the DSN from the
/// config (`[telemetry].sentry_dsn`), else the `SENTRY_DSN` env var, else the
/// built-in fallback so crash reporting works out of the box. The returned guard
/// flushes events on drop, so `main` must hold it.
fn init_telemetry(config_dsn: Option<String>) -> sentry::ClientInitGuard {
    let dsn = config_dsn
        .or_else(|| std::env::var("SENTRY_DSN").ok())
        .unwrap_or_else(|| {
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

/// `entheai --doctor`: install the rain-on-glass shader into the user's own
/// Ghostty config (viz Slice 2b, Path A) and print a health summary. Reuses the
/// launcher's bundled shader — one shader, one canonical location.
fn run_doctor_cmd() -> anyhow::Result<()> {
    use entheai_launcher::ConfigAction;
    let home = entheai_launcher::entheai_config_dir();
    let cfg = entheai_launcher::ghostty_config_path();
    let r = entheai_launcher::run_doctor(&home, &cfg)?;

    let tilde = |p: &Path| -> String {
        match std::env::var("HOME") {
            Ok(h) => match p.strip_prefix(&h) {
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => p.display().to_string(),
            },
            Err(_) => p.display().to_string(),
        }
    };

    println!("entheai doctor — rain-on-glass shader (Ghostty)\n");
    if r.is_ghostty_term {
        println!("  terminal        Ghostty ✓  (the shader renders here)");
    } else {
        println!("  terminal        not Ghostty — the shader only renders inside Ghostty");
        println!("                  (the ANSI ambient fallback, Path C, isn't built yet)");
    }
    if r.ghostty_installed {
        println!("  ghostty binary  found ✓");
    } else {
        println!("  ghostty binary  not found — install: brew install --cask ghostty");
    }
    println!("  shader          {} ✓", tilde(&r.shader_path));
    let act = match r.action {
        ConfigAction::Created => "created config + added shader block ✓",
        ConfigAction::Added => "added shader block ✓",
        ConfigAction::Updated => "updated shader block (path changed) ✓",
        ConfigAction::AlreadyCurrent => "already configured ✓ (no change)",
    };
    println!("  config          {}", tilde(&r.config_path));
    println!("                  {act}");
    println!("\n  → restart Ghostty (or reload its config) to see the rain-on-glass effect.");
    Ok(())
}

/// `entheai --skills add <url>`: fetch + install a skill, then exit. Resolves the
/// install dir from `[skills].dirs` (first entry, default "skills") under `root`.
async fn run_skills_cmd(
    args: &[String],
    cfg: &entheai_config::Config,
    root: &std::path::Path,
) -> anyhow::Result<()> {
    let dir_name = cfg.skills.dirs.first().map(String::as_str).unwrap_or("skills");
    let skills_dir = root.join(dir_name);
    match args.first().map(String::as_str) {
        Some("add") if args.len() >= 2 => {
            let url = &args[1];
            let added = entheai_skills::remote::add_from_url(url, &skills_dir).await?;
            if added.is_empty() {
                println!("skills: nothing to add from {url}");
            }
            for a in &added {
                if a.skipped_existing {
                    println!(
                        "skills: {} already exists at {} — skipping (delete to re-add)",
                        a.slug,
                        a.path.display()
                    );
                } else {
                    println!("skills: added '{}' [{}] -> {}", a.name, a.tier, a.path.display());
                }
            }
            if added.iter().any(|a| !a.skipped_existing) {
                println!("added from {url} — a skill's instructions are advisory to the agent. It's live on the next run.");
            }
            Ok(())
        }
        Some("list") if args.len() == 1 => {
            let dirs: Vec<std::path::PathBuf> =
                cfg.skills.dirs.iter().map(|d| root.join(d)).collect();
            let reg = entheai_skills::SkillRegistry::discover(&dirs);
            if reg.is_empty() {
                println!("no skills found in {:?}", cfg.skills.dirs);
            } else {
                println!("{} skill(s):", reg.list().len());
                for s in reg.list() {
                    println!("  {} — {}", s.name, s.description);
                    println!("      {}", s.path.display());
                }
            }
            Ok(())
        }
        Some("remove") if args.len() == 2 => {
            let name = &args[1];
            match entheai_skills::remote::remove_from_dir(&skills_dir, name)? {
                Some(p) => println!("skills: removed {}", p.display()),
                None => println!("skills: no skill named {name:?} in {}", skills_dir.display()),
            }
            Ok(())
        }
        _ => anyhow::bail!("usage: entheai --skills <add <url> | list | remove <name>>"),
    }
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

/// Map the config's `[obsidian]` block to the runtime options.
fn obsidian_options(o: &entheai_config::ObsidianConfig) -> entheai_obsidian::ObsidianOptions {
    entheai_obsidian::ObsidianOptions {
        enabled: o.enabled,
        vault_path: o.vault_path.clone(),
        subtree: o.subtree.clone(),
        watch: o.watch.clone(),
        debounce_ms: o.debounce_ms,
        mcp_nudge: o.mcp_nudge,
        mcp_port: o.mcp_port,
        include_architecture: o.include_architecture,
        include_sessions: o.include_sessions,
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
            entheai_memory::Embedder::new(
                pc.base_url.clone(),
                cfg.memory.embed_model.clone(),
                cfg.memory.embed_timeout_secs,
            )
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

/// Build the prompt-processing pipeline when `[memory] mode = "prompt-processing"`
/// and memory is enabled. Slice 2 selects the real subprocess seams when their
/// commands are configured (the non-empty defaults): the stdio-JSON-RPC mesh
/// sidecar and the `mq compress --semantic` compressor. Both degrade to top-K via
/// the pipeline's fail-safe when the tool is absent, so this is safe even without
/// python/`mq` installed; an empty `sidecar_cmd`/`marqant_cmd` forces the in-process
/// stub. Returns `None` for the default `topk` mode. Mirrors `build_memory`.
fn build_prompt_processor(
    cfg: &Config,
) -> anyhow::Result<Option<entheai_memory_pp::PromptProcessor>> {
    use entheai_memory_pp::{
        KompressMarqant, Marqant, MeshSearch, NativeMesh, PromptProcessor, RawStore,
        RetrievalMode, SidecarMesh, StubMarqant, StubMesh, SubprocessMarqant,
    };

    // Preview budget sent to the mesh per candidate, and how many ranked findings
    // to rehydrate. Kept modest (compression runs after) — implementation tuning.
    const PP_PREVIEW_BYTES: usize = 4096;
    const PP_MESH_TOP_K: usize = 16;

    if !cfg.memory.enabled
        || RetrievalMode::parse(&cfg.memory.mode) != RetrievalMode::PromptProcessing
    {
        return Ok(None);
    }
    let pc = cfg.memory.prompt_processing.clone().unwrap_or_default();
    let path = expand_home(&pc.raw_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let raw = RawStore::open(&path)?;

    // Stage-2 mesh backend. "native" (default) reranks in-process via
    // entheai-ultragraph — no Python. "sidecar" uses the stdio-JSON-RPC serve.py;
    // "stub" always falls back to top-K. All share the raw store (cheap clone).
    let mesh: Box<dyn MeshSearch> = match pc.mesh_backend.trim() {
        "stub" => Box::new(StubMesh),
        "sidecar" if !pc.sidecar_cmd.trim().is_empty() => {
            Box::new(SidecarMesh::new(raw.clone(), &pc.sidecar_cmd, PP_PREVIEW_BYTES, PP_MESH_TOP_K))
        }
        "sidecar" => Box::new(StubMesh), // requested but no sidecar_cmd → stub
        other => {
            if !other.is_empty() && other != "native" {
                log::warn!("unknown pp mesh_backend {other:?}; using native");
            }
            // Optional .ugm reranker; rejected/absent → the lexical scorer.
            let model = if pc.native_model.trim().is_empty() {
                None
            } else {
                let m = NativeMesh::load_model(&expand_home(&pc.native_model));
                if m.is_none() {
                    log::warn!(
                        "pp native_model {:?} missing or wrong shape; using lexical scorer",
                        pc.native_model
                    );
                }
                m
            };
            Box::new(NativeMesh::new(raw.clone(), model, PP_PREVIEW_BYTES, PP_MESH_TOP_K))
        }
    };
    let marqant: Box<dyn Marqant> = match pc.marqant_backend.trim() {
        "stub" => Box::new(StubMarqant),
        "kompress" => Box::new(KompressMarqant::new()),
        "subprocess" if pc.marqant_cmd.trim().is_empty() => {
            log::warn!("pp marqant_backend is \"subprocess\" but marqant_cmd is empty; using stub");
            Box::new(StubMarqant)
        }
        "subprocess" => Box::new(SubprocessMarqant::new(&pc.marqant_cmd)),
        other => {
            log::warn!("unknown pp marqant_backend {other:?}; using subprocess");
            if pc.marqant_cmd.trim().is_empty() {
                Box::new(StubMarqant)
            } else {
                Box::new(SubprocessMarqant::new(&pc.marqant_cmd))
            }
        }
    };

    let pp = PromptProcessor::new(
        raw,
        mesh,
        marqant,
        std::time::Duration::from_millis(pc.search_deadline_ms),
        pc.recall_k,
        pc.max_ingest_bytes,
    );
    Ok(Some(pp))
}

/// Inspect the memory store and exit. Namespaces: codebase, learnings,
/// trajectories, tools, subagents.
async fn run_memory_cmd(cfg: &Config, args: &[String]) -> anyhow::Result<()> {
    use entheai_memory::Namespace;
    let store = build_memory(cfg)?
        .ok_or_else(|| anyhow::anyhow!("memory is disabled ([memory] enabled = false)"))?;

    let parse_ns = |s: &str| -> anyhow::Result<Namespace> {
        s.parse::<Namespace>().map_err(|_| {
            anyhow::anyhow!(
                "unknown namespace '{s}' (codebase|learnings|trajectories|tools|subagents)"
            )
        })
    };

    match args.first().map(String::as_str) {
        Some("stats") => {
            let mut total = 0usize;
            for ns in [
                Namespace::Codebase,
                Namespace::Learnings,
                Namespace::Trajectories,
                Namespace::Tools,
                Namespace::Subagents,
            ] {
                let n = store.list(ns, usize::MAX, 0).await?.len();
                total += n;
                println!("{:<13} {n}", ns.as_str());
            }
            println!("{:<13} {total}", "total");
        }
        Some("list") => {
            let ns = parse_ns(args.get(1).map(String::as_str).unwrap_or(""))?;
            for e in store.list(ns, 20, 0).await? {
                let preview: String = e.content.chars().take(80).collect();
                println!(
                    "{}  {}  {}",
                    e.key,
                    e.created_at,
                    preview.replace('\n', " ")
                );
            }
        }
        Some("search") => {
            let ns = parse_ns(args.get(1).map(String::as_str).unwrap_or(""))?;
            let query = args.get(2..).map(|q| q.join(" ")).unwrap_or_default();
            if query.trim().is_empty() {
                anyhow::bail!("usage: --memory search <namespace> <query...>");
            }
            for s in store.search(ns, &query, 10).await? {
                let preview: String = s.entry.content.chars().take(80).collect();
                println!(
                    "[{:.3}] {}  {}",
                    s.score,
                    s.entry.key,
                    preview.replace('\n', " ")
                );
            }
        }
        _ => anyhow::bail!("usage: --memory <list <ns> | search <ns> <query...> | stats>"),
    }
    Ok(())
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
        let load = tokio::time::timeout(
            std::time::Duration::from_secs(cfg.mcp_defaults.spawn_timeout_secs),
            async {
                let (client, guard) = entheai_mcp::McpClient::spawn(
                    &mcp_cfg.command,
                    &mcp_cfg.args,
                    name,
                    std::time::Duration::from_secs(cfg.mcp_defaults.spawn_timeout_secs),
                )
                .await?;
                let tools = entheai_mcp::load_tools(client).await?;
                Ok::<_, entheai_mcp::McpError>((guard, tools))
            },
        )
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
            Err(_) => eprintln!(
                "mcp: '{name}' timed out after {}s — skipping",
                cfg.mcp_defaults.spawn_timeout_secs
            ),
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
        cfg.companion.port.to_string(),
        "--cwd".to_string(),
        root.display().to_string(),
        "--socket".to_string(),
        socket_path.display().to_string(),
        "--fps".to_string(),
        cfg.companion.fps.to_string(),
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
