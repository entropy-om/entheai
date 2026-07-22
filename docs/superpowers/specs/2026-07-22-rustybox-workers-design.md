# rustybox for federation workers — design

## Goal

Shrink the executable surface inside the Linux federation worker's sandbox to **one
audited, memory-safe-ish, static binary**. Today a confined coder's `run_shell` execs the
host's `/bin/sh` and reaches whatever host binaries the Landlock policy happens to allow.
Instead, provision the user's **rustybox** (a Rust BusyBox fork — one ~4 MB fully-static
musl multicall binary, 300+ applets including `ash`) into the jail and route `run_shell`
through `rustybox sh -c …`. The jail then exposes exactly one executable, downloaded from a
known release with a verified checksum, instead of the host userland.

## Edition + license (decided)

Use the **full `rustybox`** (GPL-2.0) release, not `rustybox-core` (MIT). The MIT core is
coreutils + grep/find + `timeout` — it has **no shell** (`ash` is BusyBox-lineage GPL code),
and `run_shell` needs real shell semantics (pipes, redirects). The GPL is a non-issue here:
entheai **downloads the binary at runtime and execs it as a separate process** — exactly how
it already shells out to `git` (also GPL-2.0). Runtime-exec of a standalone binary is neither
linking nor redistribution, so it imposes no obligation on entheai's own MIT/Apache licensing.
entheai never bundles rustybox in its release; the worker fetches it from
`github.com/peterlodri-sec/rustybox/releases`.

## Scope

Linux workers only (rustybox is raw-syscall Linux). macOS/other workers keep host `/bin/sh`
(current behaviour). Opt-in via config; default off — zero change unless enabled. Does **not**
touch `crates/memory` (Rahul's); the surface is `crates/tools`, `crates/sandbox`,
`bin/entheai-worker`, `crates/config`.

## The integration points (grounded)

- **`run_shell`** hardcodes the shell: `crates/tools/src/shell.rs:61` —
  `tokio::process::Command::new("/bin/sh").arg("-c")…`. `RunShell::new(dir)` builds the tool.
- **Landlock policy**: `crates/sandbox/src/linux.rs` — `apply_fs_landlock(work_dir,
  read_only_paths)` grants `AccessFs::from_all` (read+write+**execute**) under `work_dir` and
  `AccessFs::from_read` (read+**execute** — Landlock treats Execute as a read right) under each
  `read_only` path. So a binary is executable in the jail iff it lives under `work_dir` or a
  `read_only` path. seccomp is a denylist; `execve` is allowed.
- **Worker**: `bin/entheai-worker/src/main.rs` — the `--sandbox-run` child builds a
  `SandboxSpec { work_dir, read_only_paths, … }`, calls `confine(spec)` in the sync prelude
  (before the tokio runtime), then `run_coder(config, config_path, item, test_coder, &work,
  tmp)`. `run_coder` builds the coder's tool registry (incl. `RunShell`).

## Architecture

### 1. Provisioning — download + verify once, cache per node

A new `crates/tools` (or a small `worker` helper) function ensures the rustybox binary exists
at a stable per-node path, e.g. `~/.cache/entheai/rustybox-<arch>` (`x86_64`/`aarch64`):

- if absent → download `rustybox-<target>-musl` + its `.sha256` sidecar from the pinned
  release URL, verify the checksum, `chmod +x`, atomic-rename into place. (Mirrors the
  entheai Homebrew release's strip+verify discipline.)
- pin a **release tag + expected sha256** in config/consts so provisioning is reproducible and
  a supply-chain swap is caught. The `.sha256` sidecar is checked against the pinned value.

This runs once at worker startup (or first confined coder), off the hot path. On any failure
(no network, checksum mismatch) → log loudly, **fall back to host `/bin/sh`** for that node.

### 2. Making it executable in the jail

The provisioned rustybox path is added to the coder's `SandboxSpec.read_only_paths`, so
Landlock grants read+execute on it (it lives outside `work_dir`, in the node cache). One extra
entry; the coder can exec it but not modify it.

### 3. Routing `run_shell` through rustybox

`RunShell` gains an injectable shell instead of the hardcoded `/bin/sh`:

```rust
// crates/tools/src/shell.rs
pub enum Shell {
    HostSh,                    // Command::new("/bin/sh").arg("-c")  (today's behaviour)
    Rustybox(PathBuf),         // Command::new(<path>).arg("sh").arg("-c")  (standalone multicall)
}
impl RunShell {
    pub fn new(dir: &Path) -> Self { Self::with_shell(dir, Shell::HostSh) }   // unchanged default
    pub fn with_shell(dir: &Path, shell: Shell) -> Self { … }
}
```

`rustybox sh -c '<cmd>'` runs rustybox's `ash` in **standalone** mode, where the shell resolves
bare `ls`/`grep`/`find`/`timeout`/… to rustybox's own applets (BusyBox standalone-shell
behaviour) — so the whole command line stays inside the one binary, no host lookup. If a build
of rustybox lacks standalone-shell, the fallback is a jailed `bin/` dir of symlinks
(`ls`→rustybox, …) placed under `work_dir` with `PATH` set to it; the plan will detect which
and pick one. Combined output capture, the streaming cap, and the `coder_timeout` kill all stay
exactly as they are — only the argv[0] changes.

### 4. Wiring it in the worker

`run_coder` (Linux, when enabled) constructs the coder's `RunShell` with
`Shell::Rustybox(path)` and adds `path` to the sandbox's `read_only_paths` before `confine`.
Everywhere else (`RunShell::new`, macOS, disabled) is byte-identical to today.

## Configuration

```toml
[federation]
worker_toolbox = "host"   # default; "rustybox" = provision + route run_shell through rustybox (Linux only)
# rustybox_tag    = "v0.2.0"        # pinned release
# rustybox_sha256 = { x86_64 = "…", aarch64 = "…" }   # pinned checksums
```

## Fail-safe (fast, loud, no regression)

Every step degrades to host `/bin/sh`:

- provisioning fails (offline / checksum mismatch) → host shell, logged loudly.
- non-Linux worker or `worker_toolbox = "host"` → host shell (unchanged).
- rustybox exec error at run time → the shell tool surfaces the error like any command failure;
  the coder isn't wedged.

With the feature off, the confined coder path is byte-identical to today. Timeouts and output
caps are unchanged.

## Security notes

- The jail's exec surface goes from "host `/bin/sh` + whatever the policy allowed" to **one
  checksum-pinned binary the coder can execute but not write**.
- Provisioning verifies a pinned sha256 — a compromised/replaced release artifact fails closed.
- rustybox is still "bug-for-bug BusyBox" internally (transpiled C, `unsafe`), so this is a
  *surface-reduction + supply-chain-pinning* win, not a memory-safety guarantee. Honest framing
  in docs.

## Testing

- **`RunShell` shell injection:** unit-test that `Shell::HostSh` builds `/bin/sh -c` (today) and
  `Shell::Rustybox(p)` builds `p sh -c`; capture/timeout behaviour identical for both. On a host
  without rustybox, a `Shell::Rustybox(bogus)` surfaces a clean exec error (no panic).
- **Provisioning:** checksum match → returns the path; mismatch → error (→ caller falls back);
  idempotent (present binary is a no-op). Stub the download in tests (local fixture + sha).
- **Landlock (dev-cx53, `--ignored`):** extend the existing jail self-test — with the feature on,
  `rustybox sh -c 'ls'` runs inside the jail, and rustybox is executable while still read-only
  (a write to it is denied).
- **Worker E2E (dev-cx53):** a confined coder with `worker_toolbox = "rustybox"` runs a real
  `run_shell` command that resolves to rustybox applets; `worker_toolbox = "host"` unchanged.

## Scope check

One subsystem, one plan: a shell-injection seam in `crates/tools`, a provisioning helper, one
extra `read_only_paths` entry, and a config knob. Slice 1 = the injectable `RunShell` +
config + host-default (no behaviour change, fully unit-tested). Slice 2 = provisioning +
Landlock exec-allow + worker wiring, proven on dev-cx53. Slice 1 lands with zero runtime change;
Slice 2 turns it on behind the flag.
