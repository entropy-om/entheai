# Federation F2.3 — Worker Sandbox Hardening, TUI Executor Wiring & Fleet UI

**Status:** Approved (2026-07-21) · **Supersedes the F2.3 deferral in** the F2.2 plan
**Depends on:** F2.1 (distributed swarm) + F2.2 (fan-out offload), both shipped.

## Goal

Close the security gate that currently keeps `entheai-worker --serve` to *trusted nodes only*:
confine the model-generated coder so it cannot read secrets or escape its worktree, wire remote
fan-out into the interactive TUI, and make the remote fleet visible.

## Current state (from exploration — anchors the design)

- The coder runs **in-process** in the worker via `entheai_orchestrator::run_coder_once`
  (`bin/entheai-worker/src/main.rs:134`), as the worker's own uid, under a **yolo/auto-allow** policy,
  in a `tempfile::tempdir()/work` clone. **Zero isolation** beyond that temp dir being deleted afterward.
- Filesystem tools are path-jailed to the worktree with symlink-escape defense
  (`crates/tools/src/fs.rs` `resolve_in_root`), but **`run_shell` is not jailed** — it is
  `/bin/sh -c <cmd>` with only a `cwd` (`crates/tools/src/shell.rs`). Any child it spawns
  (`curl … | sh`, `cat ~/.ssh/id_rsa`) is outside the tool layer. **That is the real blast radius.**
- `securefs` has **zero code footprint** — prose only (spec §4, plans, `entheai.toml:69`,
  `CHANGELOG.md:18`). `docs/entheai-worker.md` has an *aspirational* sandbox section (Seatbelt /
  bwrap+landlock+seccomp, a `sandbox = strict|permissive|off` knob) that does not exist in code.
- The coder **makes its own LLM calls** (`run_coder_once` runs the full model+tool loop), so it needs
  outbound HTTPS to the provider — network cannot simply be cut.
- The TUI hardcodes `None` for the executor (`crates/tui/src/lib.rs:547`), so remote fan-out is
  unreachable from the interactive loop even when `[federation]` is on.
- Presence exists as `heartbeat()` + `count_workers(window)` (`crates/federation/src/lib.rs`) — it
  **counts, does not enumerate** node identities, and nothing renders it.

## Decisions

1. **Scope:** all three components — (A) sandbox hardening, (B) TUI executor wiring, (C) fleet UI.
2. **Sandbox depth:** OS-level, **Linux-first** (workers run on Linux — dev-cx53, Hetzner; the Mac is
   the dispatcher). Move the coder to a **confined child subprocess**.
3. **Network:** **filesystem + syscall confinement; leave network open** in v1 (the coder needs the
   LLM). An egress allow-list to the provider is a documented follow-up, not v1.
4. **Default mode:** `permissive` (attempt confinement; if unavailable, loud-warn + run unconfined so
   existing `--serve` setups don't break).

---

## A. Worker sandbox hardening

### A.1 Architecture — the coder runs in a self-sandboxing child

The worker process does NATS/git/object-store I/O *around* each coder run, so we cannot confine the
worker process itself. Instead the worker **spawns a child** that runs only the coder and
self-applies the sandbox before executing any model/tool code:

```
entheai-worker --sandbox-run --work <dir> --role <r> --task-file <f> --config <c>
```

- Task text is passed via a **file** (`--task-file`), not argv, to avoid arg-size/leakage.
- Provider keys reach the child via **inherited env** (the worker's env, loaded from `.env`).
- The child, at startup, **before** `run_coder_once`:
  1. **(Linux) Landlock** (ABI-negotiated, degrades gracefully): grant `<work>` = read+write+exec;
     a read-only allow-list of what a coder legitimately needs (see A.4); deny everything else.
  2. **seccomp** (`seccompiler`): set `no_new_privs` + a **denylist** of escape syscalls (`ptrace`,
     `mount`, `unshare`, `setns`, `pivot_root`, `kexec_load`, `bpf`, `add_key`, `keyctl`,
     `process_vm_readv/writev`, `perf_event_open`, …). Denylist (not allowlist) because a coder runs
     diverse tools (git, cargo); an allowlist would be too brittle and break legitimate work.
  3. **drop root** to a non-root uid/gid if the worker runs as root (via `rustix`/`nix`).
  4. Network is left open (per decision 3).
- The child then runs `run_coder_once(config, role, task, work)` in the now-confined process, exits
  `0` on success (worktree mutated in place) or non-zero on failure.
- The **parent** spawns the child with `deadline_secs` (kills on timeout), and on success runs the
  existing `commit_and_bundle_delta` over `<work>` (unchanged). On non-zero exit / timeout the parent
  **discards** the worktree (returns `None`) so a half-done edit is never bundled.

**Alternatives considered & rejected:**
- *External wrapper (`bwrap` / `sandbox-exec`)* — not always installed on a worker; less precise
  control; kept only as a possible `permissive`-tier fallback, not the primary path.
- *In-process Landlock (no child)* — would also confine the worker's own NATS/git/object-store I/O,
  which it must perform after the coder runs. The child model gives clean separation.

### A.2 `crates/sandbox` (new crate)

Single-responsibility crate so the OS-specific, cfg-gated dependencies stay isolated from federation's
NATS concern and are independently testable.

```rust
pub enum SandboxMode { Strict, Permissive, Off }      // parsed from config

pub struct SandboxSpec {
    pub work_dir: PathBuf,
    pub read_only_paths: Vec<PathBuf>,
    pub drop_uid: Option<u32>,     // and gid
}

pub enum Availability { Available, Unavailable(String) }  // reason for logs/refusal

pub fn availability() -> Availability;                 // can this host confine at all?
pub fn confine(spec: &SandboxSpec) -> Result<(), SandboxError>;  // called BY the child at startup
```

- `#[cfg(target_os = "linux")] linux.rs` — Landlock (`landlock` crate) + seccomp (`seccompiler`) +
  setuid/setgid (`rustix`). Linux deps behind a crate feature so non-Linux / `--no-default-features`
  builds don't pull them.
- `macos.rs` — best-effort `sandbox-exec` profile (fs confinement, network allowed); `Unavailable`
  if the profile can't be established.
- `fallback.rs` — always `Unavailable("unsupported OS")`.

The **mode → action** decision lives in the worker (not the crate): `Strict` + `Unavailable` ⇒ refuse;
`Permissive` + `Unavailable` ⇒ warn + run unconfined; `Off` ⇒ never attempt.

### A.3 Config

Add to `FederationConfig` (`crates/config/src/lib.rs`):

```toml
[federation]
sandbox = "permissive"   # "strict" | "permissive" | "off"  (default: permissive)
```

- `strict` — if `confine()` can't fully apply the **filesystem** ruleset (old kernel w/o Landlock,
  macOS without a working profile, not permitted), the child exits with a distinct code; the parent
  does **not** ack, so JetStream redelivers / the dispatcher's existing per-coder **local fallback**
  runs it on the origin box. Nothing runs unconfined. (A missing *network* rule is not a refusal —
  network is open anyway.)
- `permissive` (default) — attempt; on `Unavailable`, log a loud one-line warning and run unconfined.
- `off` — never attempt (today's behavior).

`--serve` prints the active mode + a one-line security posture at startup. Also **document the unread
`federation.role` knob as reserved** (worker mode is chosen by the `--serve`/`--dispatch` CLI flags,
not config — wiring it is out of scope here) and fix the stale artifacts: `entheai.toml:69`
("securefs hardening is F2.2" → F2.3, + document `sandbox`) and the aspirational section of
`docs/entheai-worker.md` (replace with the shipped behavior).

### A.4 Read-only allow-list (Linux Landlock)

What a coder legitimately needs beyond `<work>` (all read-only unless noted):

- the `entheai-worker` binary + its loader (`/usr`, `/lib`, `/lib64`, `/bin`),
- CA certificates + `/etc/resolv.conf`, `/etc/hosts` (TLS + DNS to the provider),
- the config file passed via `--config`,
- **only if** `[fanout] verify` shells out to cargo: the rustc/cargo toolchain + `~/.cargo`, `~/.rustup`,
- `/tmp` (read+write) and `/dev/null`, `/dev/urandom`.

Everything else — `~/.ssh`, other repos, `~/.aws`, `/etc/shadow`, the rest of `$HOME` — is denied.

---

## B. TUI executor wiring

- Add `entheai-federation` as a dependency of `crates/tui` (matching the default-feature gating the
  bin uses).
- Build `fed_exec: Option<Arc<dyn CoderExecutor>>` **once at TUI startup**, exactly as
  `bin/entheai/src/main.rs:150-164` does (gate on `cfg.federation.enabled` → `Federation::connect(…)`
  → `FederationExecutor::new(f, root)`), and clone it into each fan-out run.
- Replace the hardcoded `None` at `crates/tui/src/lib.rs:547` with that `fed_exec.clone()`.

Result: the interactive TUI offloads fan-out coders to the fleet when `[federation]` is enabled and a
worker is serving, with the same presence-gated per-coder local fallback the CLI already has.

## C. Fleet UI

- **Presence payload gains identity.** Extend the heartbeat message to carry
  `{ node_id, hostname, version, state, current_task?, started_at }`, where `node_id` reuses the
  FNV-seeded machine-id scheme already used by the TUI env banner (consistency), and `state` is
  `idle | working`.
- **Enumeration API.** Add `list_workers(window) -> Vec<WorkerPresence>` alongside `count_workers`
  (collect distinct node payloads observed within the window).
- **`/fleet` slash command** (read-only) renders the roster:

```
fleet · 3 nodes (800ms window)
 ● cx53·2d14   dev-cx53   0.2.1  working  "coder#1: add tests"  1s
 ● hz·9f0a     hetzner    0.2.1  idle                           2s
 ○ mac·7c24    this-mac   0.2.1  idle (dispatcher)              0s
```

Reuses the federation connection from (B). **Remote stop/kill is out of scope for v1** — local
`/workers` still manages local workers; `/fleet` is visibility only.

---

## Error handling & fallback (summary)

| Situation | Behavior |
|---|---|
| Sandbox `Unavailable`, mode `strict` | child exits distinct code → parent no-ack → dispatcher local fallback on origin |
| Sandbox `Unavailable`, mode `permissive` | loud warning → run unconfined |
| Child crash / non-zero exit / deadline timeout | parent kills, **discards** worktree (returns `None`) → local fallback; never bundles a half-done edit |
| Landlock ABI older than built-for | negotiate best-effort; under `strict` refuse only if the *filesystem* ruleset can't apply |

## Security threat model & residual risks

**Contained in v1:** reading host secrets / other repos / `$HOME` (Landlock fs jail); worktree escape;
privilege escalation (`no_new_privs`, drop-root, escape-syscall denylist); a shell child inheriting the
confinement (it's the same confined process tree).

**NOT contained (documented residual):**
- **Network egress** — the coder can still reach arbitrary hosts / the local network / cloud metadata
  (`169.254.169.254`) and exfiltrate (it can also exfil via the LLM). Mitigation = the egress-allowlist
  follow-up.
- **Resource exhaustion** beyond the existing caps (bundle 128 MiB, shell output cap + 120 s timeout,
  10 MiB file reads) — no cgroup/rlimit in v1.
- **`permissive` on a Landlock-less kernel** runs unconfined by design (loudly logged).

**Non-goals:** full VM/container/microVM isolation; remote worker kill from the TUI; egress filtering.

## Testing

- **Unit:** `SandboxMode` parse + default; `SandboxSpec`/read-only-list construction; the
  mode×availability → refuse/warn/skip decision table.
- **Linux integration (gated; run on dev-cx53):** a `--sandbox-run` self-test that, once confined,
  **proves the jail** — reading `/etc/shadow` or a path outside `<work>` returns `EACCES`; a
  denylisted syscall is blocked; writing inside `<work>` and reaching the network still succeed.
  Skipped where Landlock is unavailable.
- **Fallback:** a strict-refusing worker → assert the dispatcher falls back to origin.
- **E2E on dev-cx53:** real `--serve` with `sandbox = "strict"`, dispatch a coder, confirm it completes
  and the fs-escape attempt is denied in logs.

## File plan

**Create**
- `crates/sandbox/` — `Cargo.toml`, `src/lib.rs`, `src/linux.rs`, `src/macos.rs`, `src/fallback.rs`.
- `docs/superpowers/plans/2026-07-21-federation-f2.3-*.md` (from writing-plans).

**Modify**
- `bin/entheai-worker/src/main.rs` — `--sandbox-run` child entry; `process_one` spawns the confined child.
- `bin/entheai-worker/Cargo.toml`, `crates/tui/Cargo.toml`, root `Cargo.toml` (new member) — deps.
- `crates/config/src/lib.rs` — `sandbox: SandboxMode` on `FederationConfig`; document `role` as reserved.
- `crates/federation/src/lib.rs` — presence payload identity + `list_workers`.
- `crates/tui/src/lib.rs` — executor wiring (`:547`); `/fleet` command + renderer.
- `entheai.toml`, `docs/entheai-worker.md`, `CHANGELOG.md` — document `sandbox`, fix stale F2.2 labels.

## Out of scope / follow-ups (F2.4+)

- Egress allow-list to the provider (network restriction).
- Remote worker stop/kill from `/fleet`.
- cgroup/rlimit resource caps on the coder child.
- Shared-remote transport (option b) and dedicated multi-worker load tests.
