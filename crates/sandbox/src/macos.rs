//! macOS best-effort confinement via `sandbox_init` (deprecated but present):
//! `(allow default)` with targeted denies — no writes outside the worktree/tmp,
//! no reads of common secret dirs. Network open. Weaker than Linux; for local
//! `--serve` testing, not production.

use std::ffi::{CStr, CString};

use crate::{Availability, SandboxError, SandboxSpec};

extern "C" {
    fn sandbox_init(
        profile: *const libc::c_char,
        flags: u64,
        errorbuf: *mut *mut libc::c_char,
    ) -> libc::c_int;
    fn sandbox_free_error(errorbuf: *mut libc::c_char);
}

pub fn availability() -> Availability {
    Availability::Available // sandbox_init present on all supported macOS
}

pub fn confine(spec: &SandboxSpec) -> Result<(), SandboxError> {
    let profile = build_profile(spec)?;
    let c = CString::new(profile).map_err(|e| SandboxError::Apply(format!("profile: {e}")))?;
    let mut err: *mut libc::c_char = std::ptr::null_mut();
    let rc = unsafe { sandbox_init(c.as_ptr(), 0, &mut err) };
    if rc != 0 {
        let msg = if err.is_null() {
            "sandbox_init failed".into()
        } else {
            let s = unsafe { CStr::from_ptr(err) }
                .to_string_lossy()
                .into_owned();
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

/// Backslash-escape `\` (first) then `"` so a path containing either can't break
/// out of a `(subpath "…")` string literal or widen the profile. A security
/// profile builder must never emit a malformed/over-broad rule from an odd path.
fn sbpl_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn build_profile(spec: &SandboxSpec) -> Result<String, SandboxError> {
    // Fail closed: an empty/unset HOME would turn the secret denies into
    // `(subpath "/.ssh")` — misdirected — while still returning Ok, silently
    // dropping the protection. Refuse instead so the caller decides (strict
    // refuses; permissive warns + runs unconfined by choice).
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return Err(SandboxError::Apply(
            "HOME unset — cannot build secret-deny profile".into(),
        ));
    }
    let work = sbpl_quote(&spec.work_dir.display().to_string());
    let home = sbpl_quote(&home);
    let sub = |p: &str| format!("(subpath \"{home}/{p}\")");
    Ok(format!(
        "(version 1)\n(allow default)\n\
         (deny file-write* (subpath \"/\"))\n\
         (allow file-write* (subpath \"{work}\") (subpath \"/tmp\") (subpath \"/private/tmp\") \
         (subpath \"/private/var/folders\") (literal \"/dev/null\") (literal \"/dev/urandom\"))\n\
         (deny file-read* {ssh} {aws} {gnupg} {gcloud})\n",
        ssh = sub(".ssh"),
        aws = sub(".aws"),
        gnupg = sub(".gnupg"),
        gcloud = sub(".config/gcloud"),
    ))
}

fn drop_privileges(uid: u32, gid: u32) -> Result<(), SandboxError> {
    unsafe {
        if libc::geteuid() != 0 {
            return Ok(());
        }
        // Drop root's supplementary groups before setgid/setuid so the confined
        // process can't retain group membership it shouldn't have.
        if libc::setgroups(0, std::ptr::null()) != 0 {
            return Err(SandboxError::Apply("setgroups failed".into()));
        }
        if libc::setgid(gid) != 0 || libc::setuid(uid) != 0 {
            return Err(SandboxError::Apply("setuid/setgid failed".into()));
        }
    }
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
    const ERR_ESCAPE_ALLOWED: i32 = 4;
    const ERR_SECRET_READ_ALLOWED: i32 = 5;

    /// Post-fork CHILD work. Kept minimal: `confine()` (unavoidable alloc) plus
    /// bare `std::fs` syscalls, mapping outcomes to an exit code. No `println!`
    /// (locks/buffers) and no `std::process::exit` (atexit) — the caller uses
    /// `libc::_exit`.
    fn child_body(spec: &crate::SandboxSpec, inside: &Path, escape: &Path, secret: &Path) -> i32 {
        if crate::confine(spec).is_err() {
            return ERR_CONFINE;
        }
        // 1. write INSIDE the worktree must still succeed.
        if std::fs::write(inside, b"ok").is_err() {
            return ERR_INSIDE_WRITE;
        }
        // 2. write OUTSIDE (under $HOME) must be denied.
        if std::fs::write(escape, b"escaped").is_ok() {
            return ERR_ESCAPE_ALLOWED;
        }
        // 3. reading a secret path must be denied.
        if std::fs::read(secret).is_ok() {
            return ERR_SECRET_READ_ALLOWED;
        }
        OK
    }

    #[test]
    fn sbpl_quote_escapes_backslash_then_quote() {
        // backslash escaped first, then quote — a lone `"` can't close the literal.
        assert_eq!(super::sbpl_quote(r#"a"b"#), r#"a\"b"#);
        assert_eq!(super::sbpl_quote(r"a\b"), r"a\\b");
        // `\"` must become `\\\"` (escape the `\`, then escape the `"`),
        // not `\\"` which would still terminate the SBPL string.
        assert_eq!(super::sbpl_quote("\\\""), "\\\\\\\"");
        assert_eq!(super::sbpl_quote("/plain/path"), "/plain/path");
    }

    // Irreversible + multi-threaded harness => fork() and confine only the child.
    // Ignored by default; run with `cargo test -p entheai-sandbox -- --ignored`.
    #[test]
    #[ignore = "irreversible confinement; forks. macOS only, run with --ignored"]
    fn confine_blocks_escape_and_secret_read() {
        let home = PathBuf::from(std::env::var("HOME").expect("HOME set"));

        // Unique temp worktree under $TMPDIR (/private/var/folders on macOS).
        // Canonicalize so the SBPL `(subpath ...)` matches the resolved path.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let work = std::env::temp_dir().join(format!("entheai-sbx-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&work).expect("create work dir");
        let work = std::fs::canonicalize(&work).expect("canonicalize work dir");

        // Fake secret under a denied subpath ($HOME/.ssh). Create the dir only if
        // absent; the probe file is always created (and always cleaned up) so the
        // deny is proven by a PermissionDenied — never a false pass on NotFound.
        let ssh_dir = home.join(".ssh");
        let created_ssh = !ssh_dir.exists();
        std::fs::create_dir_all(&ssh_dir).expect("create .ssh");
        let secret = ssh_dir.join("entropy-f23-secret-probe.txt");
        std::fs::write(&secret, b"TOP-SECRET-F23").expect("write secret probe");

        let escape = home.join("entropy-f23-escape.txt");
        let _ = std::fs::remove_file(&escape); // clean pre-state
        let inside = work.join("entropy-f23-inside.txt");

        let spec = crate::SandboxSpec {
            work_dir: work.clone(),
            read_only_paths: vec![],
            drop_uid: None,
        };

        // fork(): only the calling thread is cloned. Everything the child needs is
        // already computed above.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            let code = child_body(&spec, &inside, &escape, &secret);
            unsafe { libc::_exit(code) };
        }

        // PARENT: reap the child.
        let mut status: libc::c_int = 0;
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(waited, pid, "waitpid failed");

        // Best-effort cleanup regardless of outcome.
        let _ = std::fs::remove_file(&secret);
        if created_ssh {
            let _ = std::fs::remove_dir(&ssh_dir);
        }
        let _ = std::fs::remove_file(&escape);
        let _ = std::fs::remove_dir_all(&work);

        assert!(
            libc::WIFEXITED(status),
            "child did not exit normally (status={status})"
        );
        let code = libc::WEXITSTATUS(status);
        assert_eq!(
            code, OK,
            "child failed (exit {code}): \
             2=confine-returned-Err 3=inside-write-BLOCKED \
             4=escape-write-ALLOWED 5=secret-read-ALLOWED"
        );
    }
}
