//! Git-bundle transport for F2: move a repo (and a coder's delta) between the
//! dispatcher and a worker as self-contained bundles. All git runs via
//! `tokio::process::Command` (mirrors the orchestrator's worktree helpers).
use std::path::Path;

async fn git(dir: &Path, args: &[&str]) -> anyhow::Result<(bool, String)> {
    let out = tokio::process::Command::new("git").arg("-C").arg(dir).args(args).output().await
        .map_err(|e| anyhow::anyhow!("spawn git -C {} {:?}: {e}", dir.display(), args))?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}
async fn git_ok(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = tokio::process::Command::new("git").arg("-C").arg(dir).args(args).output().await?;
    if !out.status.success() {
        anyhow::bail!("git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Create a base bundle of `repo`'s HEAD under `out` (a `.bundle` path). Bundles
/// the branch `fed-base` pointing at HEAD so a clone lands on a named branch.
pub async fn bundle_base(repo: &Path, out: &Path) -> anyhow::Result<String> {
    let base_sha = git_ok(repo, &["rev-parse", "HEAD"]).await?.trim().to_string();
    // A fresh branch ref for the bundle (force in case it exists).
    git_ok(repo, &["branch", "-f", "fed-base", &base_sha]).await?;
    let out_s = out.to_string_lossy();
    git_ok(repo, &["bundle", "create", &out_s, "fed-base"]).await?;
    // The bundle is self-contained now; don't leave a `fed-base` branch littering
    // the dispatcher's real repo (best-effort — a failure here is harmless).
    let _ = git(repo, &["branch", "-D", "fed-base"]).await;
    Ok(base_sha)
}

/// Clone a base bundle into `dest` and check out `fed-base`, then create a
/// working branch `fed-work`. Returns the worktree path (`dest`).
pub async fn materialize_from_bundle(bundle: &Path, dest: &Path) -> anyhow::Result<()> {
    let bundle_s = bundle.to_string_lossy();
    let dest_s = dest.to_string_lossy();
    let out = tokio::process::Command::new("git")
        .args(["clone", "-b", "fed-base", &bundle_s, &dest_s]).output().await?;
    if !out.status.success() {
        anyhow::bail!("git clone bundle failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    git_ok(dest, &["checkout", "-b", "fed-work"]).await?;
    // Identity so commits succeed in the ephemeral clone.
    git_ok(dest, &["config", "user.email", "worker@entheai"]).await?;
    git_ok(dest, &["config", "user.name", "entheai-worker"]).await?;
    Ok(())
}

/// After a coder changed files in `worktree`: stage+commit; if nothing changed
/// return Ok(None); else bundle `base_sha..fed-work` to `out` and return the
/// new sha.
pub async fn commit_and_bundle_delta(worktree: &Path, base_sha: &str, msg: &str, out: &Path) -> anyhow::Result<Option<String>> {
    git_ok(worktree, &["add", "-A"]).await?;
    let (clean, _) = git(worktree, &["diff", "--cached", "--quiet"]).await?;
    if clean { return Ok(None); } // nothing staged
    git_ok(worktree, &["commit", "-m", msg]).await?;
    let new_sha = git_ok(worktree, &["rev-parse", "HEAD"]).await?.trim().to_string();
    let out_s = out.to_string_lossy();
    let range = format!("{base_sha}..fed-work");
    git_ok(worktree, &["bundle", "create", &out_s, &range]).await?;
    Ok(Some(new_sha))
}

/// In the dispatcher's `repo` (which has `base_sha`), fetch the worker's delta
/// bundle into a fresh local branch `branch`. Returns the fetched tip sha.
pub async fn apply_delta_bundle(repo: &Path, bundle: &Path, branch: &str) -> anyhow::Result<String> {
    let bundle_s = bundle.to_string_lossy();
    let refspec = format!("fed-work:refs/heads/{branch}");
    git_ok(repo, &["fetch", &bundle_s, &refspec]).await?;
    Ok(git_ok(repo, &["rev-parse", branch]).await?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn init_repo(dir: &Path) {
        git_ok(dir, &["init", "-q"]).await.unwrap();
        git_ok(dir, &["config", "user.email", "t@t"]).await.unwrap();
        git_ok(dir, &["config", "user.name", "t"]).await.unwrap();
        tokio::fs::write(dir.join("README.md"), "base\n").await.unwrap();
        git_ok(dir, &["add", "-A"]).await.unwrap();
        git_ok(dir, &["commit", "-q", "-m", "base"]).await.unwrap();
    }

    #[tokio::test]
    async fn full_bundle_round_trip_applies_the_delta() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = tmp.path().join("disp");
        tokio::fs::create_dir_all(&dispatcher).await.unwrap();
        init_repo(&dispatcher).await;

        // Dispatcher bundles base.
        let base_bundle = tmp.path().join("base.bundle");
        let base_sha = bundle_base(&dispatcher, &base_bundle).await.unwrap();
        // bundle_base must not leave a `fed-base` branch in the dispatcher's repo.
        let (has_fed_base, _) = git(&dispatcher, &["rev-parse", "--verify", "fed-base"]).await.unwrap();
        assert!(!has_fed_base, "fed-base branch should be cleaned up after bundling");

        // Worker materializes, changes a file, delta-bundles.
        let work = tmp.path().join("work");
        materialize_from_bundle(&base_bundle, &work).await.unwrap();
        assert_eq!(tokio::fs::read_to_string(work.join("README.md")).await.unwrap(), "base\n");
        tokio::fs::write(work.join("NEW.md"), "from worker\n").await.unwrap();
        let result_bundle = tmp.path().join("result.bundle");
        let new_sha = super::commit_and_bundle_delta(&work, &base_sha, "worker change", &result_bundle).await.unwrap();
        assert!(new_sha.is_some());

        // Dispatcher applies the delta to a branch.
        let tip = apply_delta_bundle(&dispatcher, &result_bundle, "fed/test").await.unwrap();
        assert_eq!(tip, new_sha.unwrap());
        // The new file exists on that branch.
        let show = git_ok(&dispatcher, &["show", "fed/test:NEW.md"]).await.unwrap();
        assert_eq!(show, "from worker\n");
    }

    #[tokio::test]
    async fn no_change_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let disp = tmp.path().join("disp");
        tokio::fs::create_dir_all(&disp).await.unwrap();
        init_repo(&disp).await;
        let base_bundle = tmp.path().join("b.bundle");
        let base_sha = bundle_base(&disp, &base_bundle).await.unwrap();
        let work = tmp.path().join("w");
        materialize_from_bundle(&base_bundle, &work).await.unwrap();
        let rb = tmp.path().join("r.bundle");
        assert!(commit_and_bundle_delta(&work, &base_sha, "noop", &rb).await.unwrap().is_none());
    }
}
