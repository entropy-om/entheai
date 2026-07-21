# Federation F2.3 — Worker Sandbox Hardening, TUI Wiring & Fleet UI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Confine the model-generated coder on a federation worker (so it cannot read host secrets or escape its worktree), wire remote fan-out into the interactive TUI, and surface the remote fleet.

**Architecture:** The worker runs each coder in a **self-sandboxing child process** (`entheai-worker --sandbox-run …`) that applies a Linux Landlock filesystem jail + a seccomp syscall denylist + drops root before running `run_coder_once`. A new `crates/sandbox` crate owns the OS-specific confinement behind a portable API. The TUI gets the same `FederationExecutor` the CLI already uses. Presence heartbeats gain node identity so a read-only `/fleet` command can list the swarm.

**Tech Stack:** Rust (edition 2021, MSRV 1.80). New crate deps (Linux-gated): `landlock` 0.4.5, `seccompiler` 0.5.0, `nix` 0.31 (feature `user` — for the uid drop; `rustix` deliberately omits `setuid`/`setgid`), `libc` 0.2. macOS-gated: `libc` (for `sandbox_init` FFI). Existing: `tokio`, `serde`, `thiserror` (lib crates) / `anyhow` (bins), `clap`, `ratatui`, `async-nats`. All new deps verified MSRV-1.80 compatible.

**Spec:** `docs/superpowers/specs/2026-07-21-federation-f2.3-worker-hardening-design.md`.

**Sequencing:** Part A (sandbox) → Part B (TUI wiring) → Part C (fleet UI). Each part builds + tests green on its own.

---

## File Structure

**Create**
- `crates/sandbox/Cargo.toml`, `crates/sandbox/src/lib.rs` — portable API (`SandboxMode`, `SandboxSpec`, `Availability`, `confine`).
- `crates/sandbox/src/linux.rs` — Landlock + seccomp + drop-root (`#[cfg(target_os = "linux")]`).
- `crates/sandbox/src/macos.rs` — `sandbox-exec` profile (`#[cfg(target_os = "macos")]`).
- `crates/sandbox/src/fallback.rs` — `Unavailable` for other targets.

**Modify**
- root `Cargo.toml` — add `crates/sandbox` workspace member.
- `crates/config/src/lib.rs` — `sandbox: SandboxMode` on `FederationConfig`.
- `bin/entheai-worker/src/main.rs` + `Cargo.toml` — `--sandbox-run` child; `process_one` spawns it.
- `crates/federation/src/lib.rs` — presence payload identity + `list_workers`.
- `crates/tui/src/lib.rs` + `Cargo.toml` — executor wiring; `/fleet` command + renderer.
- `entheai.toml`, `docs/entheai-worker.md`, `CHANGELOG.md` — document `sandbox`, fix stale F2.2 labels.

---

## Part A — Worker sandbox hardening

### Task A1: Scaffold `crates/sandbox` with the portable API

**Files:**
- Create: `crates/sandbox/Cargo.toml`
- Create: `crates/sandbox/src/lib.rs`
- Create: `crates/sandbox/src/fallback.rs`
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Add the crate to the workspace.** In root `Cargo.toml`, add `"crates/sandbox"` to `[workspace] members` (keep the list alphabetized if it is).

- [ ] **Step 2: Write `crates/sandbox/Cargo.toml`.**

```toml
[package]
name = "entheai-sandbox"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
thiserror = { workspace = true }
serde = { workspace = true, features = ["derive"] }

# Linux-only confinement backends (no-op deps elsewhere).
[target.'cfg(target_os = "linux")'.dependencies]
landlock = "0.4.5"
seccompiler = "0.5.0"
nix = { version = "0.31", default-features = false, features = ["user"] }
libc = "0.2"

# macOS best-effort backend (sandbox_init FFI).
[target.'cfg(target_os = "macos")'.dependencies]
libc = "0.2"
```

- [ ] **Step 3: Write the failing test for `SandboxMode` parsing.** In `crates/sandbox/src/lib.rs` add a `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mode_parses_and_defaults() {
        assert_eq!(SandboxMode::parse("strict"), Some(SandboxMode::Strict));
        assert_eq!(SandboxMode::parse("  Permissive "), Some(SandboxMode::Permissive));
        assert_eq!(SandboxMode::parse("off"), Some(SandboxMode::Off));
        assert_eq!(SandboxMode::parse("nope"), None);
        assert_eq!(SandboxMode::default(), SandboxMode::Permissive);
    }
}
```

- [ ] **Step 4: Run it to confirm it fails.** `cargo test -p entheai-sandbox` → FAIL (`SandboxMode` not defined).

- [ ] **Step 5: Implement the portable API** in `crates/sandbox/src/lib.rs`:

```rust
//! Portable process-confinement for the federation worker's coder child.
//! `confine()` is called by the `entheai-worker --sandbox-run` child at startup,
//! before any model/tool code runs; it is irreversible for the calling process.

use std::path::PathBuf;

/// Confinement posture, from `[federation] sandbox`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Refuse to run the coder if confinement can't be applied.
    Strict,
    /// Attempt confinement; if unavailable, warn and run unconfined (default).
    #[default]
    Permissive,
    /// Never attempt confinement (today's behavior).
    Off,
}

impl SandboxMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "strict" => Some(Self::Strict),
            "permissive" => Some(Self::Permissive),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// What the child asks `confine()` to enforce.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    /// The one writable+executable directory (the coder's worktree).
    pub work_dir: PathBuf,
    /// Paths granted read+execute (toolchain, CA certs, config, …).
    pub read_only_paths: Vec<PathBuf>,
    /// If the process is root, drop to this (uid, gid). `None` = skip.
    pub drop_uid: Option<(u32, u32)>,
}

/// Whether this host can confine at all (cheap probe, no side effects).
#[derive(Debug)]
pub enum Availability {
    Available,
    Unavailable(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("sandbox unavailable on this host: {0}")]
    Unavailable(String),
    #[error("failed to apply sandbox: {0}")]
    Apply(String),
}

/// Probe whether confinement is possible here (kernel/OS support).
pub fn availability() -> Availability {
    imp::availability()
}

/// Apply the sandbox to the CURRENT process/thread. Irreversible. Call once,
/// at child startup, before running any untrusted code.
pub fn confine(spec: &SandboxSpec) -> Result<(), SandboxError> {
    imp::confine(spec)
}

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[path = "fallback.rs"]
mod imp;
```

- [ ] **Step 6: Write the fallback backend** `crates/sandbox/src/fallback.rs`:

```rust
use crate::{Availability, SandboxError, SandboxSpec};

pub fn availability() -> Availability {
    Availability::Unavailable("confinement unsupported on this OS".into())
}

pub fn confine(_spec: &SandboxSpec) -> Result<(), SandboxError> {
    Err(SandboxError::Unavailable("confinement unsupported on this OS".into()))
}
```

- [ ] **Step 7: Stub the OS backends so the crate builds on every target.** Create `crates/sandbox/src/linux.rs` and `crates/sandbox/src/macos.rs` each with a temporary body identical to `fallback.rs` (the real bodies land in Task A3/A4). This keeps `cargo build` green on all hosts now.

- [ ] **Step 8: Run tests + clippy.** `cargo test -p entheai-sandbox` → PASS; `cargo clippy -p entheai-sandbox` → clean. (On macOS the `macos.rs` stub compiles; the real profile lands in A4.)

- [ ] **Step 9: Commit.** `git add crates/sandbox Cargo.toml && git commit -m "feat(sandbox): scaffold entheai-sandbox crate (portable confine API)"`

---

### Task A2: Add `sandbox` to `FederationConfig`

**Files:**
- Modify: `crates/config/src/lib.rs` (the `FederationConfig` struct + its `Default`, and `crates/config/Cargo.toml`)
- Test: inline `#[cfg(test)]` in `crates/config/src/lib.rs`

- [ ] **Step 1: Add the dependency.** In `crates/config/Cargo.toml` add `entheai-sandbox = { path = "../sandbox" }`.

- [ ] **Step 2: Write the failing test.** Add to the config tests module:

```rust
#[test]
fn federation_sandbox_defaults_permissive_and_parses() {
    use entheai_sandbox::SandboxMode;
    let cfg: Config = toml::from_str("").unwrap();
    assert_eq!(cfg.federation.sandbox, SandboxMode::Permissive); // default

    let cfg: Config = toml::from_str("[federation]\nsandbox = \"strict\"\n").unwrap();
    assert_eq!(cfg.federation.sandbox, SandboxMode::Strict);

    assert!(toml::from_str::<Config>("[federation]\nsandbox = \"bogus\"\n").is_err());
}
```

- [ ] **Step 3: Run it → FAIL** (`no field sandbox`). `cargo test -p entheai-config federation_sandbox`.

- [ ] **Step 4: Add the field.** In `FederationConfig` add:

```rust
    /// Coder confinement posture on this worker (see crates/sandbox).
    #[serde(default)]
    pub sandbox: entheai_sandbox::SandboxMode,
```

If `FederationConfig` has a hand-written `Default` impl (not `#[derive(Default)]`), add `sandbox: entheai_sandbox::SandboxMode::default(),` to it. Verify with `rg "impl Default for FederationConfig"`.

- [ ] **Step 5: Run tests → PASS.** `cargo test -p entheai-config` (fix any struct-literal construction of `FederationConfig` elsewhere in the crate/tests to include the new field).

- [ ] **Step 6: Commit.** `git add crates/config && git commit -m "feat(config): [federation] sandbox = strict|permissive|off (default permissive)"`

---

### Task A3: Linux backend — Landlock + seccomp + drop-root

**Files:**
- Modify: `crates/sandbox/src/linux.rs` (replace the A1 stub)

APIs pinned against **landlock 0.4.5, seccompiler 0.5.0, nix 0.31** (verified current). Key facts:
`restrict_self()` sets `no_new_privs` and restricts the **calling thread + threads created after it**;
`seccompiler::apply_filter` also sets `no_new_privs` and the filter is inherited by later threads
(so confining once, single-threaded, before the runtime starts, covers all tokio threads — see A5).
BestEffort compatibility is the default (old kernels drop unsupported bits rather than erroring).

- [ ] **Step 1: Write the failing gated self-test.** In `crates/sandbox/src/linux.rs`, a `#[test] #[ignore]`
  that **forks** (confinement is irreversible per-process) and, in the child, `confine()`s to a temp
  work dir then asserts: (a) `std::fs::write(work/"ok", …)` succeeds; (b) `File::open("/etc/shadow")`
  errors with `PermissionDenied`; (c) `nix::sched::unshare(CLONE_NEWUSER)` errors with `EPERM`. Parent
  `waitpid`s and asserts the child exited 0. (Provide the fork/waitpid harness with `nix`.)

- [ ] **Step 2: Run it → FAIL** (stub `confine` returns `Unavailable`). `cargo test -p entheai-sandbox -- --ignored` on Linux.

- [ ] **Step 3: Implement `crates/sandbox/src/linux.rs`:**

```rust
//! Linux confinement: Landlock filesystem jail + seccomp syscall denylist +
//! permanent privilege drop. Applied to the CURRENT thread while the process is
//! still single-threaded — the `--sandbox-run` child calls this BEFORE building
//! its async runtime, so tokio threads spawned afterward inherit the Landlock
//! domain and the seccomp filter.

use std::collections::BTreeMap;
use std::convert::TryInto;
use std::path::{Path, PathBuf};

use landlock::{
    Access, AccessFs, CompatLevel, Compatible, RestrictionStatus, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetError, RulesetStatus, ABI, path_beneath_rules,
};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter};

use crate::{Availability, SandboxError, SandboxSpec};

pub fn availability() -> Availability {
    match Ruleset::default().handle_access(AccessFs::from_all(ABI::V1)) {
        Ok(_) => Availability::Available,
        Err(e) => Availability::Unavailable(format!("landlock: {e}")),
    }
}

pub fn confine(spec: &SandboxSpec) -> Result<(), SandboxError> {
    let status = apply_fs_landlock(&spec.work_dir, &spec.read_only_paths)
        .map_err(|e| SandboxError::Apply(format!("landlock: {e}")))?;
    if matches!(status.ruleset, RulesetStatus::NotEnforced) {
        return Err(SandboxError::Unavailable(
            "landlock not enforced by this kernel".into(),
        ));
    }
    apply_seccomp_denylist().map_err(|e| SandboxError::Apply(format!("seccomp: {e}")))?;
    if let Some((uid, gid)) = spec.drop_uid {
        drop_privileges(uid, gid).map_err(|e| SandboxError::Apply(format!("drop: {e}")))?;
    }
    Ok(())
}

/// rwx under `work_dir`, read+execute under existing `read_only` paths, deny the
/// rest. BestEffort downgrades unsupported ABI bits on old kernels.
fn apply_fs_landlock(work_dir: &Path, read_only: &[PathBuf]) -> Result<RestrictionStatus, RulesetError> {
    let abi = ABI::V5;
    let ro: Vec<PathBuf> = read_only.iter().filter(|p| p.exists()).cloned().collect();
    Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        .add_rules(path_beneath_rules([work_dir], AccessFs::from_all(abi)))?
        .add_rules(path_beneath_rules(ro.iter(), AccessFs::from_read(abi)))?
        .restrict_self()
}

/// Deny escape syscalls with EPERM; allow everything else. Sets no_new_privs.
fn apply_seccomp_denylist() -> Result<(), Box<dyn std::error::Error>> {
    let denied: &[libc::c_long] = &[
        libc::SYS_ptrace, libc::SYS_mount, libc::SYS_umount2, libc::SYS_unshare,
        libc::SYS_setns, libc::SYS_pivot_root, libc::SYS_chroot, libc::SYS_kexec_load,
        libc::SYS_init_module, libc::SYS_finit_module, libc::SYS_delete_module,
        libc::SYS_bpf, libc::SYS_add_key, libc::SYS_keyctl, libc::SYS_request_key,
        libc::SYS_process_vm_readv, libc::SYS_process_vm_writev, libc::SYS_perf_event_open,
    ];
    let rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> =
        denied.iter().map(|&nr| (nr as i64, Vec::new())).collect();
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,                     // default: allow
        SeccompAction::Errno(libc::EPERM as u32), // matched syscall -> EPERM
        std::env::consts::ARCH.try_into()?,       // TargetArch at runtime
    )?;
    let program: BpfProgram = filter.try_into()?;
    seccompiler::apply_filter(&program)?;
    Ok(())
}

/// Permanently drop root to (uid, gid); no-op if not effectively root. gid/groups
/// before uid (can't change them once uid is dropped).
fn drop_privileges(uid: u32, gid: u32) -> nix::Result<()> {
    use nix::unistd::{geteuid, setgroups, setresgid, setresuid, Gid, Uid};
    if !geteuid().is_root() {
        return Ok(());
    }
    let (u, g) = (Uid::from_raw(uid), Gid::from_raw(gid));
    setgroups(&[])?;
    setresgid(g, g, g)?;
    setresuid(u, u, u)?;
    Ok(())
}
```

- [ ] **Step 4: Run the self-test on dev-cx53** — `cargo test -p entheai-sandbox -- --ignored` → all three assertions PASS. **This is the security proof; do not skip.** Also `cargo clippy -p entheai-sandbox` on Linux.

- [ ] **Step 5: Commit.** `git commit -m "feat(sandbox): Linux backend — Landlock fs-jail + seccomp denylist + drop-root"`

---

### Task A4: macOS backend — best-effort `sandbox_init` profile

**Files:**
- Modify: `crates/sandbox/src/macos.rs` (replace the A1 stub)

macOS is best-effort (local `--serve` testing; real workers are Linux). Uses the deprecated-but-present
`sandbox_init` with an `(allow default)` profile plus targeted denies — no writes outside the worktree,
no reads of common secret dirs — so the process (dyld, git, cargo) still runs. Network stays open.

- [ ] **Step 1: Write the failing macOS-gated self-test** (`#[test] #[ignore]`, forked): after `confine()`,
  assert writing under `work` succeeds, writing `$HOME/escape.txt` fails, and reading `$HOME/.ssh` (create
  a temp fake) fails. Parent waits and asserts child exit 0.

- [ ] **Step 2: Implement `crates/sandbox/src/macos.rs`:**

```rust
//! macOS best-effort confinement via `sandbox_init` (deprecated but present):
//! `(allow default)` with targeted denies — no writes outside the worktree/tmp,
//! no reads of common secret dirs. Network open. Weaker than Linux; for local
//! `--serve` testing, not production.

use std::ffi::{CStr, CString};

use crate::{Availability, SandboxError, SandboxSpec};

extern "C" {
    fn sandbox_init(profile: *const libc::c_char, flags: u64, errorbuf: *mut *mut libc::c_char) -> libc::c_int;
    fn sandbox_free_error(errorbuf: *mut libc::c_char);
}

pub fn availability() -> Availability {
    Availability::Available // sandbox_init present on all supported macOS
}

pub fn confine(spec: &SandboxSpec) -> Result<(), SandboxError> {
    let profile = build_profile(spec);
    let c = CString::new(profile).map_err(|e| SandboxError::Apply(format!("profile: {e}")))?;
    let mut err: *mut libc::c_char = std::ptr::null_mut();
    let rc = unsafe { sandbox_init(c.as_ptr(), 0, &mut err) };
    if rc != 0 {
        let msg = if err.is_null() {
            "sandbox_init failed".into()
        } else {
            let s = unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned();
            unsafe { sandbox_free_error(err) };
            s
        };
        return Err(SandboxError::Apply(msg));
    }
    if let Some((uid, gid)) = spec.drop_uid {
        drop_privileges(uid, gid)?;
    }
    Ok(())
}

fn build_profile(spec: &SandboxSpec) -> String {
    let work = spec.work_dir.display();
    let home = std::env::var("HOME").unwrap_or_default();
    let sub = |p: &str| format!("(subpath \"{home}/{p}\")");
    format!(
        "(version 1)\n(allow default)\n\
         (deny file-write* (subpath \"/\"))\n\
         (allow file-write* (subpath \"{work}\") (subpath \"/tmp\") (subpath \"/private/tmp\") \
         (subpath \"/private/var/folders\") (literal \"/dev/null\") (literal \"/dev/urandom\"))\n\
         (deny file-read* {ssh} {aws} {gnupg} {gcloud})\n",
        ssh = sub(".ssh"), aws = sub(".aws"), gnupg = sub(".gnupg"), gcloud = sub(".config/gcloud"),
    )
}

fn drop_privileges(uid: u32, gid: u32) -> Result<(), SandboxError> {
    unsafe {
        if libc::geteuid() != 0 {
            return Ok(());
        }
        if libc::setgid(gid) != 0 || libc::setuid(uid) != 0 {
            return Err(SandboxError::Apply("setuid/setgid failed".into()));
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Run + iterate on THIS Mac** — `cargo test -p entheai-sandbox -- --ignored`. If the profile
  is too strict (dyld/git break) or too loose (a deny assertion fails), adjust the SBPL and re-run until
  the three assertions pass. (macOS is best-effort; the profile is a verified-on-hardware starting point.)

- [ ] **Step 4: Commit.** `git commit -m "feat(sandbox): macOS best-effort backend (sandbox_init profile)"`

---

### Task A5: Worker spawns the coder in a `--sandbox-run` child

**Files:**
- Modify: `bin/entheai-worker/src/main.rs`
- Modify: `bin/entheai-worker/Cargo.toml` (add `entheai-sandbox = { path = "../../crates/sandbox" }`)

**Context:** Today `process_one` (main.rs ~120-135) calls `entheai_orchestrator::run_coder_once(config, role, task, &work).await` in-process. This task moves that call into a confined child of the same binary. The parent keeps materialize → (spawn child) → `commit_and_bundle_delta` → upload.

- [ ] **Step 1: Add a hidden CLI mode.** In the `clap` args struct add:

```rust
/// Internal: run one coder confined in this process, then exit. Not for direct use.
#[arg(long, hide = true)]
sandbox_run: bool,
#[arg(long, requires = "sandbox_run")]
work: Option<std::path::PathBuf>,
#[arg(long, requires = "sandbox_run")]
role: Option<String>,
#[arg(long, requires = "sandbox_run")]
task_file: Option<std::path::PathBuf>,
```

- [ ] **Step 2: Handle `--sandbox-run` BEFORE the async runtime starts.** Confinement must be applied while the process is **single-threaded** — `restrict_self()` and `apply_filter` cover the calling thread plus threads created *after* them, so tokio worker/blocking threads spawned later inherit the domain + filter only if we confine first. If the worker's `main` is `#[tokio::main]`, convert it to a manual `fn main()` that branches before entering any runtime:

```rust
fn main() -> anyhow::Result<()> {
    // logging / dotenv init that does NOT spawn threads is fine here
    let cli = Cli::parse();
    if cli.sandbox_run {
        return run_sandboxed_coder_blocking(cli); // confines, then a current-thread rt
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve_or_dispatch(cli)) // the existing async body, moved into this fn
}
```

- [ ] **Step 3: Implement `run_sandboxed_coder_blocking`** — read the task, apply confinement per the mode *while single-threaded*, then run exactly one coder on a **current-thread** runtime (which pre-spawns no worker threads; its blocking-pool threads are created lazily, after `confine()`):

```rust
/// Child entry. Exit 0 = coder ran (worktree mutated in place); 3 = sandbox
/// refused (strict + unavailable); other non-zero = coder error.
fn run_sandboxed_coder_blocking(cli: Cli) -> anyhow::Result<()> {
    let config = entheai_config::Config::load(cli.config.as_deref())?; // match existing load
    let work = cli.work.clone().context("missing --work")?;
    let role = cli.role.clone().context("missing --role")?;
    let task = std::fs::read_to_string(cli.task_file.as_ref().context("missing --task-file")?)?;

    let spec = entheai_sandbox::SandboxSpec {
        work_dir: work.clone(),
        read_only_paths: sandbox_read_only_paths(&config, cli.config.as_deref()),
        drop_uid: None, // privilege drop wired in A6
    };
    match config.federation.sandbox {
        entheai_sandbox::SandboxMode::Off => eprintln!("[worker] sandbox=off — coder UNCONFINED"),
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

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(entheai_orchestrator::run_coder_once(&config, &role, &task, &work))?;
    Ok(())
}
```

`sandbox_read_only_paths(&config, cli.config.as_deref())` returns the A.4 allow-list — the config path, `/usr`,`/lib`,`/lib64`,`/bin`,`/etc/ssl`,`/etc/ca-certificates`,`/etc/resolv.conf`,`/etc/hosts`,`/tmp`, and — only if `config.fanout.verify.is_some()` — `~/.cargo`,`~/.rustup` — filtered to paths that exist. (Privilege-drop via `worker_drop_uid()` is wired in A6.)

- [ ] **Step 4: Parent spawns the child instead of calling the coder in-process.** In `process_one`, replace the `None =>` arm's `run_coder_once(...)` call with:

```rust
let task_file = tmp.path().join("task.txt");
std::fs::write(&task_file, &item.task)?;
let exe = std::env::current_exe()?;
let mut cmd = tokio::process::Command::new(&exe);
cmd.arg("--sandbox-run")
    .arg("--work").arg(&work)
    .arg("--role").arg(&item.role)
    .arg("--task-file").arg(&task_file);
if let Some(cfg) = config_path { cmd.arg("--config").arg(cfg); }
let status = tokio::time::timeout(deadline, cmd.status())
    .await
    .map_err(|_| anyhow::anyhow!("coder child timed out"))??;
let log = if status.success() {
    "coder ok".to_string()
} else if status.code() == Some(3) {
    // strict-refused: leave unacked so the dispatcher's local fallback runs it.
    anyhow::bail!("sandbox strict-refused on this worker");
} else {
    anyhow::bail!("coder child failed: {status}");
};
```

(The exact `deadline`/`config_path` names come from the surrounding `process_one`; match them. The `--test-coder` path is unchanged.) On any bail, `process_one` returns `Err`, the item is **not acked**, and JetStream redelivery / the dispatcher fallback covers it — matching the existing failure contract.

- [ ] **Step 5: Guard result integrity.** After a failed child (bail), ensure `commit_and_bundle_delta` is NOT called (the early `Err` return already skips it). Confirm a half-edited worktree is never bundled.

- [ ] **Step 6: Build + existing worker tests.** `cargo build -p entheai-worker` and `cargo test -p entheai-worker`. Add a unit test for `sandbox_read_only_paths` (returns only existing paths; includes the config path; excludes cargo dirs when `verify` is None).

- [ ] **Step 7: Commit.** `git add bin/entheai-worker && git commit -m "feat(worker): run each coder in a confined --sandbox-run child"`

---

### Task A6: Mode posture at startup + reconcile docs/knobs

**Files:**
- Modify: `bin/entheai-worker/src/main.rs` (`run_serve` startup log + `worker_drop_uid`)
- Modify: `entheai.toml`, `docs/entheai-worker.md`, `CHANGELOG.md`
- Modify: `crates/config/src/lib.rs` (doc-comment `role` as reserved)

- [ ] **Step 1: Implement `worker_drop_uid` and wire it in.** Returns `Some((uid, gid))` from `SUDO_UID`/`SUDO_GID` or the `nobody` uid/gid when `geteuid()==0`, else `None`. Then change `run_sandboxed_coder_blocking`'s `SandboxSpec { drop_uid: None }` (from A5) to `drop_uid: worker_drop_uid()`. Add a unit test that it returns `None` when not root.

- [ ] **Step 2: Print the posture at `--serve` startup.** In `run_serve`, after connecting, log one line: `worker serving · sandbox={mode} · confinement={available|unavailable: reason}` using `entheai_sandbox::availability()`. Under `strict` + unavailable, log a prominent warning that real coders will refuse.

- [ ] **Step 3: Reconcile `entheai.toml`.** In the `[federation]` block: fix the comment `securefs hardening is F2.2` → `F2.3`, and add:
```toml
sandbox = "permissive"     # coder confinement: "strict" | "permissive" | "off"
# role is reserved; worker mode is chosen by --serve / --dispatch.
```

- [ ] **Step 4: Reconcile `docs/entheai-worker.md`.** Replace the aspirational "Sandboxing" section with the shipped behavior: Linux Landlock fs-jail + seccomp denylist + drop-root, network open (residual risk), the `sandbox` modes, and the read-only allow-list. Remove the invented `worker.toml sandbox` knob reference; point at `[federation] sandbox`.

- [ ] **Step 5: Document `role` as reserved** in the `FederationConfig` doc comment.

- [ ] **Step 6: Build + commit.** `cargo build -p entheai-worker && git add bin/entheai-worker entheai.toml docs/entheai-worker.md crates/config && git commit -m "docs(worker): document sandbox modes + posture log; reconcile stale F2.2 labels"`

---

## Part B — TUI executor wiring

### Task B1: Build `FederationExecutor` in the TUI and pass it to `run_fanout`

**Files:**
- Modify: `crates/tui/Cargo.toml` (add `entheai-federation = { path = "../federation" }`)
- Modify: `crates/tui/src/lib.rs`

**Context:** `bin/entheai/src/main.rs:150-164` builds `fed_exec: Option<Arc<dyn CoderExecutor>>` from `[federation]`. The TUI hardcodes `None` at `crates/tui/src/lib.rs:547`.

- [ ] **Step 1: Add the dep.** `entheai-federation = { path = "../federation" }` in `crates/tui/Cargo.toml`, matching the feature-gating the bin uses (if the bin gates federation behind a feature, gate the TUI usage identically).

- [ ] **Step 2: Build `fed_exec` once at TUI startup.** In `run`/`event_loop` where `config`, `root` are in scope (near the other run-scoped setup), mirror the bin:

```rust
let fed_exec: Option<std::sync::Arc<dyn entheai_orchestrator::CoderExecutor>> =
    if config.federation.enabled {
        match entheai_federation::Federation::connect(&config).await {
            Ok(f) => Some(std::sync::Arc::new(
                entheai_federation::FederationExecutor::new(f, root.clone()))),
            Err(e) => { tracing::warn!("federation off ({e}); local coders"); None }
        }
    } else { None };
```

(Match the exact `Federation::connect` / `FederationExecutor::new` signatures used at `bin/entheai/src/main.rs:150-164`.)

- [ ] **Step 3: Pass it into the run.** At `crates/tui/src/lib.rs:547`, replace `None` with `fed_exec.clone()`.

- [ ] **Step 4: Build both feature sets.** `cargo build -p entheai-tui` and `cargo build -p entheai --no-default-features` (confirm the headless build still links — gate the federation import if needed).

- [ ] **Step 5: Commit.** `git add crates/tui && git commit -m "feat(tui): wire FederationExecutor into fan-out (remote offload from the TUI)"`

---

## Part C — Fleet UI

### Task C1: Presence payload gains node identity + `list_workers`

**Files:**
- Modify: `crates/federation/src/lib.rs`
- Test: inline `#[cfg(test)]`

**Context:** `heartbeat()` publishes to `PRESENCE_SUBJECT`; `count_workers(window)` counts. This adds identity + enumeration.

- [ ] **Step 1: Write the failing test** for a round-trip serialize/deserialize of the new `WorkerPresence` payload (id, hostname, version, state, current_task, started_at) and that `list_workers` dedups by `node_id`.

- [ ] **Step 2: Define `WorkerPresence`** (serde struct) and change `heartbeat()` to publish it as JSON. `node_id` = FNV-1a of the hostname → 6 hex (mirror the TUI env-banner `seeded_machine_id`; extract it to a shared helper or duplicate the tiny function with a comment linking the two). `version` = `env!("CARGO_PKG_VERSION")`. `state` = an enum `{ Idle, Working { task: String } }` the worker updates.

- [ ] **Step 3: Add `list_workers(window) -> Vec<WorkerPresence>`.** Subscribe to `PRESENCE_SUBJECT`, collect messages for `window`, dedup by `node_id` keeping the newest. Keep `count_workers` working (can delegate to `list_workers().len()`).

- [ ] **Step 4: Worker publishes real state.** In `bin/entheai-worker`, thread the current role/task into the heartbeat (Idle between claims, `Working { task }` during `process_one`).

- [ ] **Step 5: Tests + build.** `cargo test -p entheai-federation && cargo build -p entheai-worker`.

- [ ] **Step 6: Commit.** `git add crates/federation bin/entheai-worker && git commit -m "feat(federation): presence payload with node identity + list_workers"`

---

### Task C2: `/fleet` slash command + renderer

**Files:**
- Modify: `crates/tui/src/lib.rs`

**Context:** Slash commands follow the `is_*_command` / `handle_*_command` + `SLASH_COMMANDS` + dispatch pattern (see `/workers`). `/fleet` is read-only.

- [ ] **Step 1: Write the failing test** — `is_fleet_command("/fleet")` true; `slash_matches("/fl")` includes `/fleet`.

- [ ] **Step 2: Add `/fleet` to `SLASH_COMMANDS`** (`"/fleet", "show the remote worker fleet (read-only)"`) and `is_fleet_command`.

- [ ] **Step 3: `handle_fleet_command`** — if `fed_exec`/federation is off, echo "federation disabled"; else call `list_workers(~800ms)` on the federation handle and format the roster (id · host · version · state · last-seen), pushing a `Role::Tool` message. Reuse the federation handle from B1 (store it on `App` or pass it into `event_loop`).

- [ ] **Step 4: Dispatch + gate.** Add the `Action::Submit(text) if is_fleet_command(&text)` arm; add `is_fleet_command` to the mid-run `is_local_command` set (read-only, safe).

- [ ] **Step 5: Tests + build.** `cargo test -p entheai-tui && cargo build -p entheai-tui`.

- [ ] **Step 6: Commit.** `git add crates/tui && git commit -m "feat(tui): /fleet — read-only remote worker roster"`

---

## Final

- [ ] **Changelog.** Add an F2.3 entry under `[Unreleased]` in `CHANGELOG.md` (sandbox hardening, TUI wiring, `/fleet`).
- [ ] **Full build + test.** `cargo build && cargo build --no-default-features && cargo test --workspace` (all green; note the Linux integration tests are `#[cfg_attr(not(target_os="linux"), ignore)]`).
- [ ] **dev-cx53 E2E (Linux, real).** rsync + build on dev-cx53, run `entheai-worker --serve` with `sandbox = "strict"`, dispatch a coder from the Mac, confirm it completes and the jail-proving self-test's fs-escape attempt is denied in the worker logs. (See memory `dev-cx53-sandbox` for the run recipe.)
- [ ] **Final review** — dispatch a code reviewer over the whole F2.3 diff (security-sensitive: focus on the confinement being applied before any untrusted code, the strict-refusal fallback path, and the read-only allow-list not being over-broad).
