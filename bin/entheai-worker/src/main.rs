use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use entheai_config::Config;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path to the entheai TOML config (same format as the main binary's).
    #[arg(long, default_value = "entheai.toml")]
    config: String,
    /// The sub-agent role (routes to a model via `[agents.<role>]`).
    #[arg(long)]
    role: Option<String>,
    /// The sub-agent's task description.
    #[arg(long)]
    task: Option<String>,
    /// Path to the isolated git worktree this coder should run against.
    #[arg(long)]
    worktree: Option<PathBuf>,
    /// Run as a worker: pull WorkItems from the federation queue and process them.
    #[arg(long)]
    serve: bool,
    /// Dispatch a single coder task to the federation queue and apply the result.
    #[arg(long)]
    dispatch: bool,
    /// For testing: replace the LLM coder with a shell command run in the worktree.
    #[arg(long)]
    test_coder: Option<String>,
    /// Internal: run one coder confined in this process, then exit. Not for direct
    /// use — the `--serve` parent spawns itself with this to sandbox each coder.
    #[arg(long, hide = true)]
    sandbox_run: bool,
    /// The worktree the confined coder mutates in place (with `--sandbox-run`).
    #[arg(long, requires = "sandbox_run")]
    work: Option<PathBuf>,
    /// File holding the coder's task text (with `--sandbox-run`).
    #[arg(long, requires = "sandbox_run")]
    task_file: Option<PathBuf>,
}

/// Whether `output` (a coder's captured result text) indicates the sub-agent
/// failed, mirroring `entheai_orchestrator::run_coder_once`'s error-capture
/// convention (`"error: coder failed: {e}"`).
fn is_error_output(output: &str) -> bool {
    output.starts_with("error:")
}

/// Render a coder's result as the single JSON line printed to stdout.
fn render_result(role: &str, task: &str, output: &str) -> String {
    serde_json::json!({ "role": role, "task": task, "output": output }).to_string()
}

/// Per-worker presence: the immutable node identity (stamped once at startup) plus
/// the current [`WorkerState`], shared with the heartbeat tasks behind a `Mutex`.
/// The lock is never held across an `.await`, so `std::sync::Mutex` is correct here.
struct Presence {
    node_id: String,
    hostname: String,
    started_at_unix: u64,
    state: std::sync::Mutex<entheai_federation::WorkerState>,
}

impl Presence {
    /// Detect this node's identity once: hostname → node_id, start time = now.
    fn detect() -> Self {
        let hostname = worker_hostname();
        let node_id = entheai_federation::seeded_node_id(&hostname);
        let started_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            node_id,
            hostname,
            started_at_unix,
            state: std::sync::Mutex::new(entheai_federation::WorkerState::Idle),
        }
    }

    /// Build a heartbeat snapshot: the fixed identity + a copy of the current state.
    fn snapshot(&self) -> entheai_federation::WorkerPresence {
        entheai_federation::WorkerPresence {
            node_id: self.node_id.clone(),
            hostname: self.hostname.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            state: self
                .state
                .lock()
                .map(|g| g.clone())
                .unwrap_or(entheai_federation::WorkerState::Idle),
            started_at_unix: self.started_at_unix,
        }
    }

    /// Set the current worker state (reflected by the next heartbeat).
    fn set(&self, state: entheai_federation::WorkerState) {
        if let Ok(mut g) = self.state.lock() {
            *g = state;
        }
    }
}

/// This host's plain hostname — the same source `seeded_node_id` hashes and the TUI
/// env-banner's `seeded_machine_id` uses. Falls back to "localhost".
fn worker_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// Worker mode: block on the federation work-queue, materialize each `WorkItem`'s
/// repo from its base bundle, run the coder in an isolated dir, bundle the delta
/// back through the object store, and publish a `WorkResult`. Runs forever.
async fn run_serve(
    config: &Config,
    config_path: &str,
    test_coder: Option<&str>,
) -> anyhow::Result<()> {
    let opts = entheai_federation::FedOptions::from_config(&config.nats, &config.federation);
    let fed = entheai_federation::Federation::connect(&opts).await.ok_or_else(|| {
        anyhow::anyhow!("federation not available (check [federation].enabled + [nats] creds)")
    })?;
    log::info!("entheai-worker: serving the coder work-queue");

    // Startup posture: one line reporting the active confinement mode and whether
    // this host can actually enforce it — so an operator sees at a glance whether
    // coders will run confined (and, under strict, whether they'll run at all).
    let sandbox_mode = config.federation.sandbox;
    let mode = match sandbox_mode {
        entheai_sandbox::SandboxMode::Strict => "strict",
        entheai_sandbox::SandboxMode::Permissive => "permissive",
        entheai_sandbox::SandboxMode::Off => "off",
    };
    match entheai_sandbox::availability() {
        entheai_sandbox::Availability::Available => {
            log::info!("worker serving · sandbox={mode} · confinement=available");
        }
        entheai_sandbox::Availability::Unavailable(reason)
            if sandbox_mode == entheai_sandbox::SandboxMode::Strict =>
        {
            log::warn!(
                "worker serving · sandbox=strict · confinement=unavailable: {reason} \
                 — real coders will REFUSE to run on this host until confinement is available"
            );
        }
        entheai_sandbox::Availability::Unavailable(reason) => {
            log::info!("worker serving · sandbox={mode} · confinement=unavailable: {reason}");
        }
    }

    // Presence: announce liveness (identity + live Idle/Working state) every 5s, and
    // answer presence pings promptly so a dispatcher's count_workers / list_workers
    // sees us right away. `presence` is stamped once and shared with both tasks.
    let presence = std::sync::Arc::new(Presence::detect());
    {
        let fed_hb = fed.clone();
        let pres_hb = presence.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                ticker.tick().await;
                fed_hb.heartbeat(&pres_hb.snapshot()).await;
            }
        });
        let fed_ping = fed.clone();
        let pres_ping = presence.clone();
        tokio::spawn(async move {
            if let Ok(mut pings) = fed_ping.subscribe_ping().await {
                while futures::StreamExt::next(&mut pings).await.is_some() {
                    fed_ping.heartbeat(&pres_ping.snapshot()).await;
                }
            }
        });
    }

    loop {
        let claimed = match fed.claim(std::time::Duration::from_secs(20)).await {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            // A transient JetStream error must not kill a long-running worker.
            Err(e) => {
                log::warn!("claim failed ({e}) — retrying");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };
        let item = claimed.item.clone();
        log::info!("claimed work {}::{} role={}", item.session, item.index, item.role);
        // Reflect Working state in presence while the coder runs; back to Idle after
        // (including the error path, since a failed process_one still returns here).
        presence.set(entheai_federation::WorkerState::Working {
            task: item.task.clone(),
        });
        let result = process_one(&fed, config, config_path, &item, test_coder).await.unwrap_or_else(|e| {
            entheai_federation::WorkResult {
                session: item.session.clone(),
                index: item.index,
                status: "error".into(),
                committed: false,
                result_bundle_key: String::new(),
                log: format!("error: {e}"),
            }
        });
        presence.set(entheai_federation::WorkerState::Idle);
        fed.publish_result(&result).await.ok();
        claimed.ack().await;
    }
}

/// Process one claimed work item end-to-end: fetch the base bundle, materialize a
/// worktree, run the coder (or a test shell command), and — if the coder changed
/// anything — upload the delta bundle and report `committed`.
async fn process_one(
    fed: &entheai_federation::Federation,
    config: &Config,
    config_path: &str,
    item: &entheai_federation::WorkItem,
    test_coder: Option<&str>,
) -> anyhow::Result<entheai_federation::WorkResult> {
    let tmp = tempfile::tempdir()?;
    let base_bundle = tmp.path().join("base.bundle");
    tokio::fs::write(&base_bundle, fed.get_bundle(&item.base_bundle_key).await?).await?;
    let work = tmp.path().join("work");
    entheai_federation::repo::materialize_from_bundle(&base_bundle, &work).await?;

    // Coder step: real LLM by default (run in a confined `--sandbox-run` child); a
    // shell command in test mode (unchanged).
    let log = match test_coder {
        Some(cmd) => {
            let out = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&work)
                .output()
                .await?;
            format!(
                "test-coder rc={}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout)
            )
        }
        None => {
            // Re-exec ourselves as a confined child that runs exactly one coder
            // against `work`, mutating it in place. Confinement is applied in the
            // child while it is still single-threaded, before its async runtime
            // starts (see `run_sandboxed_coder_blocking`).
            let task_file = tmp.path().join("task.txt");
            std::fs::write(&task_file, &item.task)?;
            let exe = std::env::current_exe()?;
            let deadline = std::time::Duration::from_secs(config.fanout.coder_timeout_secs);
            let mut cmd = tokio::process::Command::new(&exe);
            // kill_on_drop: when the deadline fires (or we bail), tokio drops the
            // `status()` future — this kills the child so a timed-out confined coder
            // stops burning CPU + model tokens and can't keep writing into a worktree
            // the parent is about to drop. Without it these orphans accumulate on a
            // long-lived --serve worker that periodically hits coder_timeout_secs.
            cmd.kill_on_drop(true)
                .arg("--sandbox-run")
                .arg("--work")
                .arg(&work)
                .arg("--role")
                .arg(&item.role)
                .arg("--task-file")
                .arg(&task_file)
                .arg("--config")
                .arg(config_path);
            let status = tokio::time::timeout(deadline, cmd.status())
                .await
                .map_err(|_| anyhow::anyhow!("coder child timed out"))??;
            if status.code() == Some(3) {
                // Sandbox strict + unavailable: the child refused to run the coder.
                // Bail before bundling — the worktree was never touched.
                anyhow::bail!("sandbox strict-refused on this worker");
            } else if !status.success() {
                anyhow::bail!("coder child failed: {status}");
            }
            format!("coder ran in a confined child (role={})", item.role)
        }
    };

    let result_bundle = tmp.path().join("result.bundle");
    match entheai_federation::repo::commit_and_bundle_delta(
        &work,
        &item.base_sha,
        &format!("fed: {}", item.task),
        &result_bundle,
    )
    .await?
    {
        Some(_new_sha) => {
            let key = entheai_federation::types::result_key(&item.session, item.index);
            fed.put_bundle(&key, &tokio::fs::read(&result_bundle).await?).await?;
            Ok(entheai_federation::WorkResult {
                session: item.session.clone(),
                index: item.index,
                status: "committed".into(),
                committed: true,
                result_bundle_key: key,
                log: truncate(&log),
            })
        }
        None => Ok(entheai_federation::WorkResult {
            session: item.session.clone(),
            index: item.index,
            status: "no-change".into(),
            committed: false,
            result_bundle_key: String::new(),
            log: truncate(&log),
        }),
    }
}

/// Cap a coder log at a bounded length before it travels over the result subject.
fn truncate(s: &str) -> String {
    s.chars().take(2000).collect()
}

/// Dispatcher mode: bundle the current repo, enqueue a single `WorkItem`, await a
/// worker's `WorkResult`, and apply its delta bundle to a fresh branch. Fail-safe:
/// no result within the deadline means the caller should run locally instead.
async fn run_dispatch(config: &Config, role: &str, task: &str) -> anyhow::Result<()> {
    let opts = entheai_federation::FedOptions::from_config(&config.nats, &config.federation);
    let fed = entheai_federation::Federation::connect(&opts)
        .await
        .ok_or_else(|| anyhow::anyhow!("federation not available"))?;
    let repo = std::env::current_dir()?;
    let session = uuid_like();
    let index = 0usize;

    // Bundle the repo base, upload it.
    let tmp = tempfile::tempdir()?;
    let base_bundle = tmp.path().join("base.bundle");
    let base_sha = entheai_federation::repo::bundle_base(&repo, &base_bundle).await?;
    let base_key = entheai_federation::types::base_key(&session, index);
    fed.put_bundle(&base_key, &tokio::fs::read(&base_bundle).await?).await?;

    // Subscribe BEFORE dispatch so the result isn't missed.
    let mut sub = fed.subscribe_result(&session, index).await?;
    fed.dispatch(&entheai_federation::WorkItem {
        session: session.clone(),
        index,
        role: role.into(),
        task: task.into(),
        base_bundle_key: base_key,
        base_sha: base_sha.clone(),
    })
    .await?;
    println!("dispatched {session}::{index} — awaiting a worker…");

    match fed.await_result(&mut sub).await {
        Some(r) if r.committed => {
            let rb = tmp.path().join("result.bundle");
            tokio::fs::write(&rb, fed.get_bundle(&r.result_bundle_key).await?).await?;
            let branch = format!("fed/{session}-{index}");
            let tip = entheai_federation::repo::apply_delta_bundle(&repo, &rb, &branch).await?;
            println!("worker committed → branch {branch} @ {tip}");
        }
        Some(r) => println!("worker returned status={} (no change applied)\n{}", r.status, r.log),
        None => println!("no worker result within the deadline — dispatch fell through (run locally)."),
    }
    Ok(())
}

/// A per-run identifier for the result subject. Avoids a `uuid` dep — the pid is
/// enough to keep concurrent dispatches on distinct subjects; stays `[a-z0-9]`.
fn uuid_like() -> String {
    format!("d{}", std::process::id())
}

/// Load and parse the entheai TOML config from `path`.
fn load_config(path: &str) -> anyhow::Result<Config> {
    let cfg_text =
        std::fs::read_to_string(path).with_context(|| format!("reading config {path}"))?;
    Ok(Config::from_toml_str(&cfg_text)?)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // The confined-coder child path MUST apply confinement while the process is
    // still single-threaded, so handle it BEFORE the multi-threaded runtime spins
    // up its worker threads (Landlock/seccomp/sandbox_init cover the calling
    // thread plus threads created afterward, not threads that already exist).
    if cli.sandbox_run {
        return run_sandboxed_coder_blocking(cli);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve_or_dispatch(cli))
}

/// The normal (unconfined-parent) entry: serve the work-queue, dispatch a task, or
/// run one coder in-process. Unchanged from the previous `async fn main` body.
async fn serve_or_dispatch(cli: Cli) -> anyhow::Result<()> {
    let config = load_config(&cli.config)?;

    if cli.serve {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
        return run_serve(&config, &cli.config, cli.test_coder.as_deref()).await;
    }

    if cli.dispatch {
        let role = cli.role.clone().unwrap_or_else(|| "coder".into());
        let task = cli.task.clone().ok_or_else(|| anyhow::anyhow!("--dispatch needs --task"))?;
        return run_dispatch(&config, &role, &task).await;
    }

    // One-shot mode: --role/--task/--worktree are required here.
    let (role, task, worktree) = match (cli.role, cli.task, cli.worktree) {
        (Some(role), Some(task), Some(worktree)) => (role, task, worktree),
        _ => anyhow::bail!(
            "one-shot mode needs --role, --task, and --worktree (or pass --serve / --dispatch)"
        ),
    };

    let output = entheai_orchestrator::run_coder_once(&config, &role, &task, &worktree).await;
    println!("{}", render_result(&role, &task, &output));
    if is_error_output(&output) {
        std::process::exit(1);
    }
    Ok(())
}

/// The paths a confined coder legitimately needs read/execute access to, filtered
/// to those that actually exist on this host. On macOS the profile allows reads by
/// default, so this list is consumed by the Linux (Landlock) backend; building it
/// here keeps the child's `SandboxSpec` portable across backends.
fn sandbox_read_only_paths(config: &Config, config_path: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(cfg) = config_path {
        candidates.push(cfg.to_path_buf());
    }
    for p in [
        "/usr",
        "/lib",
        "/lib64",
        "/bin",
        "/etc/ssl",
        "/etc/ca-certificates",
        "/etc/resolv.conf",
        "/etc/hosts",
        "/tmp",
    ] {
        candidates.push(PathBuf::from(p));
    }
    // A `verify` command (e.g. `cargo test`) needs the Rust toolchain caches.
    if config.fanout.verify.is_some() {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            candidates.push(home.join(".cargo"));
            candidates.push(home.join(".rustup"));
        }
    }
    candidates.into_iter().filter(|p| p.exists()).collect()
}

/// The (uid, gid) the confined coder should drop to, so untrusted model-generated
/// code never keeps root. Returns `Some` only when we are actually running as root
/// (`geteuid() == 0`): prefer the invoking user via `SUDO_UID`/`SUDO_GID` when both
/// are set and parse, else fall back to `nobody` (65534 on Linux — the conventional
/// unprivileged id). Off-root returns `None`: the `entheai_sandbox` drop is a no-op
/// there anyway, but `None` keeps the "only drop when we hold privilege" intent explicit.
fn worker_drop_uid() -> Option<(u32, u32)> {
    // SAFETY: `geteuid` is an always-succeeds syscall with no arguments or preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return None;
    }
    let sudo = std::env::var("SUDO_UID")
        .ok()
        .and_then(|u| u.trim().parse::<u32>().ok())
        .zip(
            std::env::var("SUDO_GID")
                .ok()
                .and_then(|g| g.trim().parse::<u32>().ok()),
        );
    Some(sudo.unwrap_or((65534, 65534)))
}

/// Child entry (`--sandbox-run`). Reads the task, applies confinement per the
/// configured [`SandboxMode`] while single-threaded, then runs exactly one coder
/// against the worktree on a current-thread runtime — mutating `--work` in place.
///
/// Exit codes: `0` = coder ran; `3` = sandbox strict + unavailable (refused);
/// `1` = coder reported an error (via `run_coder_once`'s captured `"error: …"`).
fn run_sandboxed_coder_blocking(cli: Cli) -> anyhow::Result<()> {
    let config = load_config(&cli.config)?;
    let work = cli.work.clone().context("missing --work")?;
    let role = cli.role.clone().context("missing --role")?;
    let task = std::fs::read_to_string(cli.task_file.as_ref().context("missing --task-file")?)?;

    let spec = entheai_sandbox::SandboxSpec {
        work_dir: work.clone(),
        read_only_paths: sandbox_read_only_paths(&config, Some(Path::new(&cli.config))),
        drop_uid: worker_drop_uid(), // never run the untrusted coder as root
    };
    match config.federation.sandbox {
        entheai_sandbox::SandboxMode::Off => {
            eprintln!("[worker] sandbox=off — coder UNCONFINED")
        }
        entheai_sandbox::SandboxMode::Permissive => {
            if let Err(e) = entheai_sandbox::confine(&spec) {
                eprintln!("[worker] sandbox unavailable ({e}); permissive → UNCONFINED");
            }
        }
        entheai_sandbox::SandboxMode::Strict => {
            if let Err(e) = entheai_sandbox::confine(&spec) {
                eprintln!("[worker] sandbox strict + unavailable ({e}); refusing");
                std::process::exit(3);
            }
        }
    }

    // `run_coder_once` never returns `Err` — a coder failure is captured as
    // `"error: …"` text, so inspect the output and exit non-zero on failure.
    let output = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(entheai_orchestrator::run_coder_once(&config, &role, &task, &work));
    if is_error_output(&output) {
        eprintln!("[worker] {output}");
        std::process::exit(1);
    }
    println!("{output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_error_output_detects_the_capture_convention() {
        assert!(is_error_output("error: coder failed: boom"));
        assert!(!is_error_output("added a null check"));
    }

    #[test]
    fn render_result_produces_valid_json_with_expected_fields() {
        let json = render_result("coder", "add x", "did the thing");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["role"], "coder");
        assert_eq!(parsed["task"], "add x");
        assert_eq!(parsed["output"], "did the thing");
    }

    #[test]
    fn worker_drop_uid_is_none_when_not_root() {
        // The test process is not root, so no privilege-drop target is chosen.
        // Guard against the rare run-as-root case (e.g. a root CI container) so the
        // assertion stays honest: worker_drop_uid only returns Some when euid == 0.
        // SAFETY: `geteuid` is an always-succeeds syscall with no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        assert_eq!(worker_drop_uid(), None);
    }

    #[test]
    fn sandbox_read_only_paths_returns_only_existing_paths() {
        let cfg = Config::from_toml_str("").unwrap();
        let paths = sandbox_read_only_paths(&cfg, None);
        // Every returned path must exist on this host (the list is filtered).
        assert!(paths.iter().all(|p| p.exists()), "unfiltered non-existent path leaked: {paths:?}");
        // A well-known always-present path must survive the filter on any Unix.
        assert!(paths.iter().any(|p| p == Path::new("/usr")), "expected /usr in {paths:?}");
    }

    #[test]
    fn sandbox_read_only_paths_includes_the_config_path_when_it_exists() {
        let cfg = Config::from_toml_str("").unwrap();
        // A real, existing file: use this test binary's own source dir marker via a
        // tempfile so the "Some + exists" branch is exercised deterministically.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let cfg_path = tmp.path();
        let paths = sandbox_read_only_paths(&cfg, Some(cfg_path));
        assert!(paths.iter().any(|p| p == cfg_path), "config path missing from {paths:?}");

        // A config path that does NOT exist must be filtered out.
        let missing = Path::new("/nonexistent/entheai-such-path-should-not-exist.toml");
        let paths = sandbox_read_only_paths(&cfg, Some(missing));
        assert!(!paths.iter().any(|p| p == missing), "non-existent config path leaked");
    }

    #[test]
    fn sandbox_read_only_paths_excludes_toolchain_dirs_without_verify() {
        // No `[fanout] verify` → the Rust toolchain caches are not requested, even
        // when they exist on this host.
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.fanout.verify.is_none());
        let paths = sandbox_read_only_paths(&cfg, None);
        if let Some(home) = std::env::var_os("HOME") {
            let cargo = PathBuf::from(&home).join(".cargo");
            let rustup = PathBuf::from(&home).join(".rustup");
            assert!(!paths.contains(&cargo), ".cargo present without verify: {paths:?}");
            assert!(!paths.contains(&rustup), ".rustup present without verify: {paths:?}");
        }
    }
}
