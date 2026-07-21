//! Linux confinement: Landlock filesystem jail + seccomp syscall denylist +
//! permanent privilege drop. Applied to the CURRENT thread while the process is
//! still single-threaded — the `entheai-worker --sandbox-run` child calls this
//! BEFORE building its async runtime, so tokio threads spawned afterward inherit
//! the Landlock domain and the seccomp filter.

use std::collections::BTreeMap;
use std::convert::TryInto;
use std::path::{Path, PathBuf};

use landlock::{
    Access, AccessFs, CompatLevel, Compatible, RestrictionStatus, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetError, RulesetStatus, ABI,
};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter};

use crate::{Availability, SandboxError, SandboxSpec};

pub fn availability() -> Availability {
    // Landlock present iff the kernel accepts a ruleset for ABI v1.
    match Ruleset::default().handle_access(AccessFs::from_all(ABI::V1)) {
        Ok(_) => Availability::Available,
        Err(e) => Availability::Unavailable(format!("landlock: {e}")),
    }
}

pub fn confine(spec: &SandboxSpec) -> Result<(), SandboxError> {
    let status = apply_fs_landlock(&spec.work_dir, &spec.read_only_paths)
        .map_err(|e| SandboxError::Apply(format!("landlock: {e}")))?;
    // BestEffort never errors on an old kernel — it silently enforces nothing.
    // Treat "not enforced at all" as unavailable so `strict` refuses rather than
    // running a coder that only *thinks* it's jailed.
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
/// rest. BestEffort downgrades unsupported ABI bits on older kernels rather than
/// failing. Restricts the calling thread + threads/execs created afterward.
fn apply_fs_landlock(
    work_dir: &Path,
    read_only: &[PathBuf],
) -> Result<RestrictionStatus, RulesetError> {
    let abi = ABI::V5;
    // A non-existent `read_only` entry would make `add_rules` error and abort the
    // whole ruleset — filter to paths that exist first.
    let ro: Vec<PathBuf> = read_only.iter().filter(|p| p.exists()).cloned().collect();
    Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        .add_rules(landlock::path_beneath_rules([work_dir], AccessFs::from_all(abi)))?
        .add_rules(landlock::path_beneath_rules(ro.iter(), AccessFs::from_read(abi)))?
        .restrict_self()
}

/// Deny a denylist of escape syscalls with EPERM; allow everything else. Also
/// sets `no_new_privs` (seccompiler's `apply_filter` does this for us). A
/// denylist (not an allowlist) keeps a coder's diverse legitimate tools working
/// (git, cargo) while closing the obvious sandbox-escape primitives.
fn apply_seccomp_denylist() -> Result<(), Box<dyn std::error::Error>> {
    let denied: &[libc::c_long] = &[
        libc::SYS_ptrace,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_kexec_load,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_bpf,
        libc::SYS_add_key,
        libc::SYS_keyctl,
        libc::SYS_request_key,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_perf_event_open,
    ];
    // Empty rule vec == unconditional match on that syscall number. `c_long` is
    // `i64` on the 64-bit Linux targets we build for, so no cast is needed (the
    // `BTreeMap<i64, _>` annotation pins the key type).
    let rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> =
        denied.iter().map(|&nr| (nr, Vec::new())).collect();
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,                     // default / mismatch: allow
        SeccompAction::Errno(libc::EPERM as u32), // matched syscall -> EPERM
        std::env::consts::ARCH.try_into()?,       // TargetArch, detected at runtime
    )?;
    let program: BpfProgram = filter.try_into()?;
    seccompiler::apply_filter(&program)?;
    Ok(())
}

/// Permanently drop root to (uid, gid); no-op if not effectively root. gid +
/// supplementary groups before uid (can't change them once uid is dropped).
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Distinct child exit codes so the parent can report *which* check failed.
    const OK: i32 = 0;
    const ERR_CONFINE: i32 = 2;
    const ERR_INSIDE_WRITE: i32 = 3;
    const ERR_ESCAPE_READ_ALLOWED: i32 = 4;
    const ERR_UNSHARE_ALLOWED: i32 = 5;

    /// Post-fork CHILD work. Minimal: `confine()` then bare `std::fs`/`libc`
    /// syscalls mapped to an exit code. No `println!` (locks) and no
    /// `std::process::exit` (atexit) — the caller uses `libc::_exit`.
    fn child_body(spec: &crate::SandboxSpec, inside: &Path, secret: &Path) -> i32 {
        if crate::confine(spec).is_err() {
            return ERR_CONFINE;
        }
        // 1. write INSIDE the worktree must still succeed (Landlock rw grant).
        if std::fs::write(inside, b"ok").is_err() {
            return ERR_INSIDE_WRITE;
        }
        // 2. reading a readable file OUTSIDE the jail must be denied by Landlock.
        //    The file is readable by permissions, so a denial proves Landlock —
        //    never a NotFound/permission false-pass.
        if std::fs::read(secret).is_ok() {
            return ERR_ESCAPE_READ_ALLOWED;
        }
        // 3. seccomp must block unshare(CLONE_NEWUSER) with EPERM.
        if unsafe { libc::unshare(libc::CLONE_NEWUSER) } == 0 {
            return ERR_UNSHARE_ALLOWED;
        }
        OK
    }

    // Irreversible + multi-threaded harness => fork() and confine only the child.
    // Ignored by default; run with `cargo test -p entheai-sandbox -- --ignored`
    // on a Landlock-capable Linux kernel.
    #[test]
    #[ignore = "irreversible confinement; forks. Linux+Landlock only, run with --ignored"]
    fn confine_blocks_escape_and_secret_read() {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir();

        let work = base.join(format!("entheai-sbx-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&work).expect("create work dir");
        let work = std::fs::canonicalize(&work).expect("canonicalize work dir");

        // A readable secret OUTSIDE the worktree, a sibling in the temp dir. It is
        // NOT under `work` and NOT in the read-only allow-list, so Landlock must
        // deny reads of it.
        let secret = base.join(format!("entheai-secret-{}-{nanos}.txt", std::process::id()));
        std::fs::write(&secret, b"TOP-SECRET-F23").expect("write secret probe");
        let secret = std::fs::canonicalize(&secret).expect("canonicalize secret");

        let inside = work.join("inside.txt");

        let spec = crate::SandboxSpec {
            work_dir: work.clone(),
            // Enough for a forked child to keep running; deliberately NOT the temp
            // dir (so the sibling secret stays denied).
            read_only_paths: vec![
                PathBuf::from("/usr"),
                PathBuf::from("/lib"),
                PathBuf::from("/lib64"),
                PathBuf::from("/bin"),
            ],
            drop_uid: None,
        };

        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            let code = child_body(&spec, &inside, &secret);
            unsafe { libc::_exit(code) };
        }

        let mut status: libc::c_int = 0;
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };

        // Best-effort cleanup in the parent (after the child is reaped).
        let _ = std::fs::remove_file(&secret);
        let _ = std::fs::remove_dir_all(&work);

        assert_eq!(waited, pid, "waitpid returned the child");
        assert!(libc::WIFEXITED(status), "child exited normally");
        assert_eq!(
            libc::WEXITSTATUS(status),
            OK,
            "confinement assertion failed (child exit: 2=confine 3=inside-write 4=escape-read-allowed 5=unshare-allowed)"
        );
    }
}
