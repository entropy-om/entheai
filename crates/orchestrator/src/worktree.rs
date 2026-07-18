//! Git-worktree plumbing for fan-out v2 (parallel coders).
//!
//! Each parallel coder sub-agent gets its own throwaway `git worktree`
//! checked out from a shared, frozen base commit so concurrent writers never
//! step on each other or on the user's main working tree. Once a coder is
//! done, [`commit_all`] snapshots its changes and [`integrate`] replays the
//! resulting branches onto a fresh integration branch (skipping any that
//! conflict) for the caller to review.
//!
//! This module is plumbing only: no LLM/agent wiring, no changes to
//! `run_fanout`. All git operations are async (`tokio::process::Command`) and
//! deterministic, so they're exercised directly against real temp git repos
//! in the test module below (no network, no mocks).

use std::path::{Path, PathBuf};

use anyhow::Context;

/// Run `git -C <dir> <args...>` and capture the outcome without failing on a
/// non-zero exit — callers decide what a failed git invocation means (e.g.
/// `git diff --cached --quiet` uses exit code as a boolean signal, not an
/// error).
async fn run_git(dir: &Path, args: &[&str]) -> anyhow::Result<(bool, String, String)> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .with_context(|| {
            format!(
                "failed to spawn `git -C {} {}`",
                dir.display(),
                args.join(" ")
            )
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status.success(), stdout, stderr))
}

/// True if `root` is inside a git work tree.
pub async fn is_git_repo(root: &Path) -> bool {
    match run_git(root, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok((true, stdout, _)) => stdout.trim() == "true",
        _ => false,
    }
}

/// Resolve a ref to a concrete commit sha (e.g. "HEAD" -> sha). Errors if not a repo.
pub async fn resolve_base(root: &Path, gitref: &str) -> anyhow::Result<String> {
    let (ok, stdout, stderr) = run_git(root, &["rev-parse", gitref]).await?;
    if !ok {
        anyhow::bail!(
            "git rev-parse {gitref} failed in {}: {stderr}",
            root.display()
        );
    }
    Ok(stdout.trim().to_string())
}

/// One isolated worktree + the branch checked out in it.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub index: usize,
}

/// Creates/removes throwaway worktrees for one fan-out session, all branched off `base`.
pub struct WorktreePool {
    root: PathBuf,
    base: String,
    session: String,
    dir: PathBuf,
}

impl WorktreePool {
    /// `base_ref` is resolved to a sha immediately (so later commits to the repo don't move it).
    /// Worktrees live under a temp dir keyed by session (NOT inside the repo).
    pub async fn new(root: &Path, session: &str, base_ref: &str) -> anyhow::Result<Self> {
        let base = resolve_base(root, base_ref).await?;
        let dir = std::env::temp_dir().join(format!("entheai-wt-{session}"));
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("failed to create worktree pool dir {}", dir.display()))?;
        Ok(Self {
            root: root.to_path_buf(),
            base,
            session: session.to_string(),
            dir,
        })
    }

    /// `git -C root worktree add -b entheai/<session>/coder-<i> <dir>/coder-<i> <base>`
    pub async fn create(&self, index: usize) -> anyhow::Result<Worktree> {
        let branch = format!("entheai/{}/coder-{}", self.session, index);
        let path = self.dir.join(format!("coder-{index}"));
        let path_str = path.to_string_lossy().into_owned();
        let (ok, _stdout, stderr) = run_git(
            &self.root,
            &["worktree", "add", "-b", &branch, &path_str, &self.base],
        )
        .await?;
        if !ok {
            anyhow::bail!(
                "git worktree add -b {branch} {path_str} {} failed: {stderr}",
                self.base
            );
        }
        Ok(Worktree {
            path,
            branch,
            index,
        })
    }

    /// `git -C root worktree remove --force <wt.path>` then `git -C root branch -D <wt.branch>`
    /// (best-effort branch delete; ignore "branch not found").
    pub async fn remove(&self, wt: &Worktree) -> anyhow::Result<()> {
        let path_str = wt.path.to_string_lossy().into_owned();
        let (ok, _stdout, stderr) =
            run_git(&self.root, &["worktree", "remove", "--force", &path_str]).await?;
        if !ok {
            anyhow::bail!("git worktree remove --force {path_str} failed: {stderr}");
        }
        // Best-effort: the branch may already be gone, or never diverged enough
        // to matter. Either way, a failed delete here shouldn't fail cleanup.
        let _ = run_git(&self.root, &["branch", "-D", &wt.branch]).await;
        Ok(())
    }

    pub fn base(&self) -> &str {
        &self.base
    }
}

/// Stage everything and commit in `path`. Returns true iff a commit was actually made
/// (false when there were no changes). Uses `-c user.email=... -c user.name=...` inline so
/// it works even if the repo has no committer configured.
pub async fn commit_all(path: &Path, message: &str) -> anyhow::Result<bool> {
    let (ok, _stdout, stderr) = run_git(path, &["add", "-A"]).await?;
    if !ok {
        anyhow::bail!("git add -A failed in {}: {stderr}", path.display());
    }

    // `diff --cached --quiet` exits 0 when the index matches HEAD (nothing
    // staged) and 1 when there's a staged diff — used here as a boolean, not
    // an error signal.
    let (nothing_staged, _stdout, _stderr) =
        run_git(path, &["diff", "--cached", "--quiet"]).await?;
    if nothing_staged {
        return Ok(false);
    }

    let (ok, _stdout, stderr) = run_git(
        path,
        &[
            "-c",
            "user.email=entheai@localhost",
            "-c",
            "user.name=entheai",
            "commit",
            "-m",
            message,
        ],
    )
    .await?;
    if !ok {
        anyhow::bail!("git commit failed in {}: {stderr}", path.display());
    }
    Ok(true)
}

/// Diff of `branch` vs `base`: `git -C root diff <base>..<branch>`. Empty string if identical.
pub async fn branch_diff(root: &Path, base: &str, branch: &str) -> anyhow::Result<String> {
    let range = format!("{base}..{branch}");
    let (ok, stdout, stderr) = run_git(root, &["diff", &range]).await?;
    if !ok {
        anyhow::bail!("git diff {range} failed in {}: {stderr}", root.display());
    }
    Ok(stdout)
}

/// Outcome of integrating coder branches onto a fresh integration branch off `base`.
#[derive(Debug, Clone)]
pub struct Integration {
    pub branch: String,
    pub merged: Vec<String>,
    pub conflicted: Vec<String>,
    pub diff: String,
}

/// Create integration branch `integration_branch` off `base` in a temp integration worktree,
/// sequentially `git merge --no-edit` each branch (on conflict: `git merge --abort` and record
/// it as conflicted), capture the combined diff, then remove the integration WORKTREE but KEEP
/// the branch (so the user can review/checkout). Never mutates the user's main working tree.
pub async fn integrate(
    root: &Path,
    base: &str,
    integration_branch: &str,
    branches: &[String],
) -> anyhow::Result<Integration> {
    let sanitized = integration_branch.replace('/', "-");
    let int_path = std::env::temp_dir().join(format!("entheai-int-{sanitized}"));
    let path_str = int_path.to_string_lossy().into_owned();

    let (ok, _stdout, stderr) = run_git(
        root,
        &["worktree", "add", "-b", integration_branch, &path_str, base],
    )
    .await?;
    if !ok {
        anyhow::bail!(
            "git worktree add -b {integration_branch} {path_str} {base} failed: {stderr}"
        );
    }

    let mut merged = Vec::new();
    let mut conflicted = Vec::new();
    for branch in branches {
        let (ok, _stdout, _stderr) = run_git(&int_path, &["merge", "--no-edit", branch]).await?;
        if ok {
            merged.push(branch.clone());
        } else {
            // Best-effort: abort whatever merge state was left behind. If
            // there was nothing to abort (e.g. the merge failed before
            // touching the index), this itself fails — that's fine, ignore it.
            let _ = run_git(&int_path, &["merge", "--abort"]).await;
            conflicted.push(branch.clone());
        }
    }

    let range = format!("{base}..HEAD");
    let (diff_ok, diff_stdout, diff_stderr) = run_git(&int_path, &["diff", &range]).await?;
    if !diff_ok {
        anyhow::bail!(
            "git diff {range} failed in {}: {diff_stderr}",
            int_path.display()
        );
    }

    let (remove_ok, _stdout, remove_stderr) =
        run_git(root, &["worktree", "remove", "--force", &path_str]).await?;
    if !remove_ok {
        anyhow::bail!("git worktree remove --force {path_str} failed: {remove_stderr}");
    }

    Ok(Integration {
        branch: integration_branch.to_string(),
        merged,
        conflicted,
        diff: diff_stdout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Run a git command and panic with git's stderr on failure — for test
    /// setup steps that must succeed for the test itself to be meaningful.
    async fn git_ok(dir: &Path, args: &[&str]) -> String {
        let (ok, stdout, stderr) = run_git(dir, args).await.expect("failed to spawn git");
        assert!(ok, "git {args:?} failed in {}: {stderr}", dir.display());
        stdout
    }

    /// `git init -b main` in a fresh tempdir, configure a local committer
    /// identity, write a known multi-line `base.txt`, and commit it. Returns
    /// the `TempDir` — keep it alive for the duration of the test.
    async fn init_repo() -> TempDir {
        let dir = TempDir::new().expect("create tempdir");
        let path = dir.path();
        git_ok(path, &["init", "-b", "main"]).await;
        git_ok(path, &["config", "user.email", "test@example.com"]).await;
        git_ok(path, &["config", "user.name", "Test User"]).await;
        tokio::fs::write(path.join("base.txt"), "line one\nline two\nline three\n")
            .await
            .expect("write base.txt");
        git_ok(path, &["add", "-A"]).await;
        git_ok(path, &["commit", "-m", "init"]).await;
        dir
    }

    #[tokio::test]
    async fn is_git_repo_true_for_repo_root() {
        let repo = init_repo().await;
        assert!(is_git_repo(repo.path()).await);
    }

    #[tokio::test]
    async fn is_git_repo_false_for_plain_dir() {
        let plain = TempDir::new().expect("create tempdir");
        assert!(!is_git_repo(plain.path()).await);
    }

    #[tokio::test]
    async fn resolve_base_returns_a_hex_sha() {
        let repo = init_repo().await;
        let sha = resolve_base(repo.path(), "HEAD")
            .await
            .expect("resolve_base");
        assert!(sha.len() >= 7, "sha too short: {sha:?}");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "not hex: {sha:?}"
        );
    }

    #[tokio::test]
    async fn create_write_commit_diff_remove_roundtrip() {
        let repo = init_repo().await;
        let root = repo.path();
        let pool = WorktreePool::new(root, "roundtrip", "HEAD")
            .await
            .expect("pool new");

        let wt = pool.create(0).await.expect("create");
        tokio::fs::write(wt.path.join("feature.txt"), "new feature\n")
            .await
            .expect("write feature.txt");

        let committed = commit_all(&wt.path, "add feature")
            .await
            .expect("commit_all");
        assert!(committed, "expected a commit to be made");

        let diff = branch_diff(root, pool.base(), &wt.branch)
            .await
            .expect("branch_diff");
        assert!(
            diff.contains("feature.txt"),
            "diff missing feature.txt: {diff}"
        );

        pool.remove(&wt).await.expect("remove");
        assert!(!wt.path.exists(), "worktree path should be gone");
    }

    #[tokio::test]
    async fn commit_all_returns_false_when_nothing_changed() {
        let repo = init_repo().await;
        let root = repo.path();
        let pool = WorktreePool::new(root, "nochange", "HEAD")
            .await
            .expect("pool new");
        let wt = pool.create(0).await.expect("create");

        let committed = commit_all(&wt.path, "noop").await.expect("commit_all");
        assert!(!committed, "expected no commit when nothing changed");

        pool.remove(&wt).await.expect("remove");
    }

    #[tokio::test]
    async fn integrate_merges_non_conflicting_branches() {
        let repo = init_repo().await;
        let root = repo.path();
        let pool = WorktreePool::new(root, "integrate-clean", "HEAD")
            .await
            .expect("pool new");

        let wt0 = pool.create(0).await.expect("create 0");
        tokio::fs::write(wt0.path.join("a.txt"), "a content\n")
            .await
            .expect("write a.txt");
        assert!(commit_all(&wt0.path, "add a").await.expect("commit a"));

        let wt1 = pool.create(1).await.expect("create 1");
        tokio::fs::write(wt1.path.join("b.txt"), "b content\n")
            .await
            .expect("write b.txt");
        assert!(commit_all(&wt1.path, "add b").await.expect("commit b"));

        let integration = integrate(
            root,
            pool.base(),
            "entheai/integrate-clean/integration",
            &[wt0.branch.clone(), wt1.branch.clone()],
        )
        .await
        .expect("integrate");

        assert_eq!(
            integration.merged.len(),
            2,
            "merged: {:?}",
            integration.merged
        );
        assert!(
            integration.conflicted.is_empty(),
            "conflicted: {:?}",
            integration.conflicted
        );
        assert!(
            integration.diff.contains("a.txt"),
            "diff: {}",
            integration.diff
        );
        assert!(
            integration.diff.contains("b.txt"),
            "diff: {}",
            integration.diff
        );

        let (verify_ok, _, _) = run_git(root, &["rev-parse", "--verify", &integration.branch])
            .await
            .expect("verify integration branch");
        assert!(verify_ok, "integration branch should exist in root repo");

        pool.remove(&wt0).await.expect("remove 0");
        pool.remove(&wt1).await.expect("remove 1");
    }

    #[tokio::test]
    async fn integrate_records_conflicts_and_leaves_main_worktree_clean() {
        let repo = init_repo().await;
        let root = repo.path();
        let pool = WorktreePool::new(root, "integrate-conflict", "HEAD")
            .await
            .expect("pool new");

        let wt0 = pool.create(0).await.expect("create 0");
        tokio::fs::write(
            wt0.path.join("base.txt"),
            "line one\nCHANGED BY ZERO\nline three\n",
        )
        .await
        .expect("write base.txt (0)");
        assert!(commit_all(&wt0.path, "conflict from 0")
            .await
            .expect("commit 0"));

        let wt1 = pool.create(1).await.expect("create 1");
        tokio::fs::write(
            wt1.path.join("base.txt"),
            "line one\nCHANGED BY ONE\nline three\n",
        )
        .await
        .expect("write base.txt (1)");
        assert!(commit_all(&wt1.path, "conflict from 1")
            .await
            .expect("commit 1"));

        let integration = integrate(
            root,
            pool.base(),
            "entheai/integrate-conflict/integration",
            &[wt0.branch.clone(), wt1.branch.clone()],
        )
        .await
        .expect("integrate");

        assert_eq!(
            integration.merged.len(),
            1,
            "merged: {:?}",
            integration.merged
        );
        assert_eq!(
            integration.conflicted.len(),
            1,
            "conflicted: {:?}",
            integration.conflicted
        );

        // The conflict happened (and was aborted) in the integration
        // worktree, which was then removed — the main worktree must be
        // untouched and clean.
        let (_, status_stdout, _) = run_git(root, &["status", "--porcelain"])
            .await
            .expect("git status");
        assert!(
            status_stdout.trim().is_empty(),
            "main worktree not clean: {status_stdout:?}"
        );

        pool.remove(&wt0).await.expect("remove 0");
        pool.remove(&wt1).await.expect("remove 1");
    }
}
