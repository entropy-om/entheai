//! karmapa-chenno — the call home. On `/freeze`, entheai publishes the
//! checkpoint plus a HUMAN-READABLE context report into a new folder of the
//! central repo and commits + pushes it herself: the operator never touches
//! git, only hand-picks folder links to share onward.
//!
//! One folder per context: `YYYY-MM-DD-<session8>/`. Repeated freezes in the
//! same session update the same folder (new checkpoint files, refreshed
//! report) — a context is one folder, however many times it froze.

use std::path::{Path, PathBuf};

/// Where one publish landed: folder name and (when derivable) the browsable URL.
pub struct Published {
    pub folder: String,
    pub url: Option<String>,
}

/// Context folder name: date first (sorts chronologically in listings), then
/// enough session id to be unique without being noise.
pub fn context_folder(session_id: &str) -> String {
    let sess: String = session_id.chars().take(8).collect();
    format!("{}-{sess}", entheai_current::utc_today())
}

/// Render the human-first context report. The brief carries the raw anchored
/// spans — the actual output a person reads; the machine state sits beside it.
pub fn render_report(
    session_id: &str,
    state: &entheai_memory_pp::EntropyState,
    brief: &str,
    hydrated: usize,
) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# 🜂 context {} — {}\n\n",
        context_folder(session_id),
        entheai_current::utc_today()
    ));
    s.push_str(&format!(
        "Session `{session_id}` · checkpoint `{}` · {hydrated}/{} span(s) held\n\n",
        state.id(),
        state.raw_span_ids.len(),
    ));
    if !state.frozen_activations.is_empty() {
        s.push_str("## Active doctrine (experience-weighted)\n\n");
        s.push_str("| node | rank |\n|---|---|\n");
        for a in &state.frozen_activations {
            s.push_str(&format!("| {} | {:.2} |\n", a.name, a.rank));
        }
        s.push('\n');
    }
    s.push_str("## The soil — anchored spans\n\n");
    if brief.is_empty() {
        s.push_str("_(no spans survived to render — the soil was empty at freeze)_\n");
    } else {
        s.push_str("```\n");
        s.push_str(brief);
        s.push_str("```\n");
    }
    s.push_str(&format!(
        "\n---\nThaw this context back into a live session: `/thaw {}`\n\
         AHOGY A DOLGOK VANNAK — as things are. Nothing more, nothing less.\n",
        state.id(),
    ));
    s
}

/// Derive a browsable folder URL from the clone's `origin` (https or ssh
/// GitHub-style remotes). `None` when the remote shape isn't recognized —
/// the publish still succeeds; only the convenience link is absent.
pub fn browse_url(remote: &str, branch: &str, folder: &str) -> Option<String> {
    let base = remote
        .trim()
        .strip_suffix(".git")
        .unwrap_or(remote.trim())
        .to_string();
    let https = if let Some(rest) = base.strip_prefix("git@") {
        // git@github.com:org/repo → https://github.com/org/repo
        let (host, path) = rest.split_once(':')?;
        format!("https://{host}/{path}")
    } else if base.starts_with("https://") || base.starts_with("http://") {
        base
    } else {
        return None;
    };
    Some(format!("{https}/tree/{branch}/{folder}"))
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("git spawn failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "git {} failed: {}{}",
            args.first().unwrap_or(&"?"),
            String::from_utf8_lossy(&out.stderr).trim(),
            stdout
        ))
    }
}

/// Publish one frozen context: write the report + checkpoint into the context
/// folder, commit, push. Errors return honestly — the local checkpoint is
/// never lost (it already exists under `.entheai/checkpoints/`), only the
/// call home failed.
pub async fn publish(
    repo_dir: &Path,
    session_id: &str,
    state: &entheai_memory_pp::EntropyState,
    brief: &str,
    hydrated: usize,
) -> Result<Published, String> {
    if !repo_dir.join(".git").exists() {
        return Err(format!(
            "chenno repo missing at {} — clone the central repo there first",
            repo_dir.display()
        ));
    }
    let folder = context_folder(session_id);
    let ctx_dir = repo_dir.join(&folder);
    std::fs::create_dir_all(&ctx_dir).map_err(|e| format!("mkdir failed: {e}"))?;
    std::fs::write(
        ctx_dir.join("README.md"),
        render_report(session_id, state, brief, hydrated),
    )
    .map_err(|e| format!("report write failed: {e}"))?;
    let ckpt_json = serde_json::to_vec_pretty(state).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(
        ctx_dir.join(format!("checkpoint-{}.json", state.id())),
        ckpt_json,
    )
    .map_err(|e| format!("checkpoint write failed: {e}"))?;

    // Pull first so parallel sessions never wedge the clone on a stale head;
    // then add → commit → push. An empty diff (re-freeze of identical state)
    // is reported as already-published, not an error.
    let _ = git(repo_dir, &["pull", "--rebase", "--quiet"]).await;
    git(repo_dir, &["add", &folder]).await?;
    let staged = git(repo_dir, &["diff", "--cached", "--name-only"]).await?;
    if staged.trim().is_empty() {
        let url = folder_url(repo_dir, &folder).await;
        return Ok(Published { folder, url });
    }
    git(
        repo_dir,
        &[
            "commit",
            "-m",
            &format!("context {folder}: checkpoint {}", state.id()),
        ],
    )
    .await?;
    git(repo_dir, &["push"]).await?;
    let url = folder_url(repo_dir, &folder).await;
    Ok(Published { folder, url })
}

async fn folder_url(repo_dir: &Path, folder: &str) -> Option<String> {
    let remote = git(repo_dir, &["remote", "get-url", "origin"]).await.ok()?;
    let branch = git(repo_dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or_else(|_| "main".into());
    browse_url(&remote, &branch, folder)
}

/// `~`-expand the configured chenno dir.
pub fn expand_dir(dir: &str) -> PathBuf {
    if let Some(rest) = dir.strip_prefix("~/") {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest)
    } else {
        PathBuf::from(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entheai_memory_pp::{EntropyState, FrozenActivation, CHECKPOINT_SCHEMA};

    fn state() -> EntropyState {
        EntropyState {
            schema: CHECKPOINT_SCHEMA.to_string(),
            session_id: "sess-abcdef123456".into(),
            created_at_ms: 1_784_800_000_000,
            frozen_activations: vec![FrozenActivation {
                name: "verification".into(),
                rank: 0.95,
            }],
            raw_span_ids: vec!["blake3:aa".into()],
            marqant_ratio: None,
            audio_seed: None,
        }
    }

    #[test]
    fn context_folder_is_date_plus_session8() {
        let f = context_folder("sess-abcdef123456");
        assert!(f.ends_with("-sess-abc"), "unexpected folder {f}");
        assert_eq!(f.len(), 10 + 1 + 8);
    }

    #[test]
    fn report_is_human_first_and_thaw_instructive() {
        let s = state();
        let r = render_report(
            &s.session_id,
            &s,
            "--- span blake3:aa ---\nhello world\n",
            1,
        );
        assert!(r.contains("# 🜂 context"));
        assert!(r.contains("| verification | 0.95 |"));
        assert!(r.contains("hello world"));
        assert!(r.contains(&format!("/thaw {}", s.id())));
        assert!(r.contains("1/1 span(s) held"));
        // Empty soil is stated, not padded.
        let empty = render_report(&s.session_id, &s, "", 0);
        assert!(empty.contains("no spans survived"));
    }

    #[test]
    fn browse_url_handles_https_ssh_and_rejects_odd_remotes() {
        assert_eq!(
            browse_url("https://github.com/entropy-om/karmapa-chenno", "main", "f").as_deref(),
            Some("https://github.com/entropy-om/karmapa-chenno/tree/main/f")
        );
        assert_eq!(
            browse_url("git@github.com:entropy-om/karmapa-chenno.git", "main", "f").as_deref(),
            Some("https://github.com/entropy-om/karmapa-chenno/tree/main/f")
        );
        assert!(browse_url("/local/path/repo", "main", "f").is_none());
    }

    #[tokio::test]
    async fn publish_commits_and_pushes_to_a_local_bare_remote() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("central.git");
        let clone = root.path().join("clone");
        for args in [
            vec!["init", "--bare", bare.to_str().unwrap()],
            vec!["clone", bare.to_str().unwrap(), clone.to_str().unwrap()],
        ] {
            assert!(std::process::Command::new("git")
                .args(&args)
                .output()
                .unwrap()
                .status
                .success());
        }
        // Identify the test committer + establish main so push has a branch.
        for args in [
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
            vec!["commit", "--allow-empty", "-m", "genesis"],
            vec!["push", "-u", "origin", "HEAD"],
        ] {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&clone)
                .args(&args)
                .output()
                .unwrap()
                .status
                .success());
        }

        let s = state();
        let published = publish(&clone, &s.session_id, &s, "--- span ---\nbody\n", 1)
            .await
            .expect("publish");
        assert!(published.folder.ends_with("sess-abc"));
        // The push landed: the bare remote's tree contains the report.
        let ls = std::process::Command::new("git")
            .arg("-C")
            .arg(&bare)
            .args(["ls-tree", "-r", "--name-only", "HEAD"])
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&ls.stdout).to_string();
        assert!(
            listing.contains(&format!("{}/README.md", published.folder)),
            "{listing}"
        );
        assert!(listing.contains(&format!("{}/checkpoint-{}.json", published.folder, s.id())));

        // Re-publishing identical state is a clean no-op (already published).
        let again = publish(&clone, &s.session_id, &s, "--- span ---\nbody\n", 1)
            .await
            .expect("re-publish");
        assert_eq!(again.folder, published.folder);

        // A missing repo reports honestly.
        let err = publish(root.path().join("nope").as_path(), &s.session_id, &s, "", 0).await;
        assert!(err.is_err());
    }
}
