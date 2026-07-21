# Federation Perf — Slice 1: Concurrent Coders on a Shared Base

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let one worker run several coders at once, each working from a single shared copy of the base repo — so throughput goes up (coders are model-wait-bound) without memory going up (they share one object store instead of one full clone each).

**Architecture:** The worker keeps a small per-node cache of base repos, one bare repo per base commit. To run a coder, it adds a cheap **detached git worktree** off the shared bare repo instead of cloning. Several coders run concurrently under a bounded limit, each in its own worktree. Every optimization is disposable: a tight deadline, an instant fallback to today's full clone on any failure, and a loud, structured signal back to the orchestrator. Proven end-to-end with a local git experiment (shared bare + N detached worktrees + `base..HEAD` bundles round-trip independently).

**Tech stack:** Rust (edition 2021, MSRV 1.80). No new crate dependencies — `tokio::sync::Semaphore` for bounded concurrency, plain `git` subprocesses (as `repo.rs` already does).

**Spec:** `docs/superpowers/specs/2026-07-21-federation-perf-design.md` (Slice 1).

---

## The proven git mechanism (what the code implements)

1. Materialize a base **once**: `git clone --bare -b entheai-fed-base <bundle> <cache>/<base_sha>.git`.
2. Per coder: `git worktree add --detach <work> entheai-fed-base` off that bare repo. The worktree's `.git` is a pointer file into the bare repo — the object store is shared.
3. Coder edits `<work>`; commit on detached HEAD.
4. Bundle the delta: `git bundle create <out> <base_sha>..HEAD` (from the worktree).
5. Dispatcher applies it: `git fetch <bundle> HEAD:refs/heads/<branch>`.

Detached worktrees mean **no branch-name collisions** between concurrent coders — that's why this is concurrency-safe. The only change to the existing pipeline is bundling/fetching `HEAD` instead of the hardcoded `fed-work`.

## File structure

**Modify**
- `crates/federation/src/repo.rs` — add worktree materialize/add/remove helpers; switch the delta bundle + fetch from `fed-work` to `HEAD`.
- `crates/federation/src/base_cache.rs` (new) — per-node base-repo cache (get-or-materialize, size cap, deadline, fail-fast).
- `crates/federation/src/lib.rs` — register the module; add a `base` outcome tag to `WorkResult`.
- `bin/entheai-worker/src/main.rs` — bounded-concurrency serve loop; wire the cache + worktree into `process_one`; per-coder fallback + degraded signal.
- `crates/config/src/lib.rs` — `[federation] max_concurrent_coders` + `base_cache_mb`.
- `entheai.toml`, `CHANGELOG.md` — document the knobs + the change.

---

## Task 1: Worktree-based materialize + HEAD-based delta in `repo.rs`

**Files:** Modify `crates/federation/src/repo.rs` (+ its `#[cfg(test)]` tests).

**Context:** Today `materialize_from_bundle` does a full `git clone` and `commit_and_bundle_delta` bundles `base_sha..fed-work`. This task adds the shared-bare + detached-worktree helpers and moves the delta to `HEAD`, so it's concurrency-safe and matches the proven mechanism. Keep `materialize_from_bundle` (it becomes the fail-fast fallback path).

- [ ] **Step 1: Write the failing test** extending the existing round-trip test to the worktree path. Add to the `tests` module:

```rust
#[tokio::test]
async fn shared_bare_two_worktrees_round_trip_independently() {
    let tmp = tempfile::tempdir().unwrap();
    let disp = tmp.path().join("disp");
    tokio::fs::create_dir_all(&disp).await.unwrap();
    init_repo(&disp).await;

    let base_bundle = tmp.path().join("base.bundle");
    let base_sha = bundle_base(&disp, &base_bundle).await.unwrap();

    // One shared bare repo; two detached worktrees (two concurrent coders).
    let bare = tmp.path().join("shared.git");
    materialize_bare(&base_bundle, &bare).await.unwrap();
    let w1 = tmp.path().join("w1");
    let w2 = tmp.path().join("w2");
    add_worktree(&bare, &w1).await.unwrap();
    add_worktree(&bare, &w2).await.unwrap();

    tokio::fs::write(w1.join("A.md"), "from-1\n").await.unwrap();
    tokio::fs::write(w2.join("B.md"), "from-2\n").await.unwrap();
    let r1 = tmp.path().join("r1.bundle");
    let r2 = tmp.path().join("r2.bundle");
    assert!(commit_and_bundle_delta(&w1, &base_sha, "c1", &r1).await.unwrap().is_some());
    assert!(commit_and_bundle_delta(&w2, &base_sha, "c2", &r2).await.unwrap().is_some());

    // Each delta applies independently on the dispatcher.
    apply_delta_bundle(&disp, &r1, "fed/1").await.unwrap();
    apply_delta_bundle(&disp, &r2, "fed/2").await.unwrap();
    assert_eq!(git_ok(&disp, &["show", "fed/1:A.md"]).await.unwrap(), "from-1\n");
    assert_eq!(git_ok(&disp, &["show", "fed/2:B.md"]).await.unwrap(), "from-2\n");

    // The worktree's .git is a pointer file (shared object store), not a dir.
    assert!(tokio::fs::metadata(w1.join(".git")).await.unwrap().is_file());

    remove_worktree(&bare, &w1).await.unwrap();
    remove_worktree(&bare, &w2).await.unwrap();
}
```

- [ ] **Step 2: Run it → FAIL** (`materialize_bare`/`add_worktree`/`remove_worktree` don't exist; the old `commit_and_bundle_delta` bundles `fed-work` not `HEAD`, so the fetch of `HEAD` in the new path finds nothing). `cargo test -p entheai-federation shared_bare`.

- [ ] **Step 3: Add the worktree helpers** to `repo.rs`:

```rust
/// Clone a base bundle into a SHARED BARE repo at `bare` (a `*.git` dir). No
/// working tree — coders attach worktrees off it and share this object store.
pub async fn materialize_bare(bundle: &Path, bare: &Path) -> anyhow::Result<()> {
    let (bundle_s, bare_s) = (bundle.to_string_lossy(), bare.to_string_lossy());
    let out = tokio::process::Command::new("git")
        .args(["clone", "--bare", "-b", "entheai-fed-base", &bundle_s, &bare_s])
        .output().await?;
    if !out.status.success() {
        anyhow::bail!("git clone --bare failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Add a detached worktree at `work` off the shared bare repo `bare`, checked out
/// at `entheai-fed-base`. Detached => no branch, so concurrent worktrees never
/// collide. Sets a commit identity so the coder's commit succeeds.
pub async fn add_worktree(bare: &Path, work: &Path) -> anyhow::Result<()> {
    let work_s = work.to_string_lossy();
    git_ok(bare, &["worktree", "add", "--detach", &work_s, "entheai-fed-base"]).await?;
    git_ok(work, &["config", "user.email", "worker@entheai"]).await?;
    git_ok(work, &["config", "user.name", "entheai-worker"]).await?;
    Ok(())
}

/// Remove a coder's worktree (and prune the admin entry) after its task. Keeps
/// the shared bare repo cached. Best-effort — a failure here is not fatal.
pub async fn remove_worktree(bare: &Path, work: &Path) -> anyhow::Result<()> {
    let work_s = work.to_string_lossy();
    let _ = git(bare, &["worktree", "remove", "--force", &work_s]).await;
    let _ = git(bare, &["worktree", "prune"]).await;
    Ok(())
}
```

- [ ] **Step 4: Move the delta to `HEAD`.** In `commit_and_bundle_delta`, change the bundle range from `fed-work` to `HEAD` (works for a detached worktree AND a branch checkout, so the fallback path keeps working):

```rust
    let range = format!("{base_sha}..HEAD");
    git_ok(worktree, &["bundle", "create", &out_s, &range]).await?;
```

And in `apply_delta_bundle`, fetch `HEAD` instead of `fed-work`:

```rust
    let refspec = format!("HEAD:refs/heads/{branch}");
    git_ok(repo, &["fetch", &bundle_s, &refspec]).await?;
```

- [ ] **Step 5: Keep the old full-clone path working as the fallback.** `materialize_from_bundle` clones + `checkout -b fed-work`; since the delta now bundles `HEAD` (the tip of whatever's checked out), leave `materialize_from_bundle` as-is — the existing `full_bundle_round_trip_applies_the_delta` test must still pass (it now exercises the fallback path with the `HEAD` bundle/fetch).

- [ ] **Step 6: Run tests + clippy → GREEN.** `cargo test -p entheai-federation` (new + existing round-trip both pass); `cargo clippy -p entheai-federation`.

- [ ] **Step 7: Commit.** `git add crates/federation/src/repo.rs && git commit -m "feat(federation): shared-bare + detached-worktree materialize; HEAD-based delta"`

---

## Task 2: Per-node base cache (`base_cache.rs`)

**Files:** Create `crates/federation/src/base_cache.rs`; register in `crates/federation/src/lib.rs`.

**Context:** A worker sees the same base commit across many coders (a fan-out). Cache the materialized bare repo per `base_sha` so we download + clone it once, not per coder. Per-node, size-bounded, and fail-fast — a cache problem must fall back to a fresh clone, never block.

- [ ] **Step 1: Write the failing test** for the pure cache bookkeeping (path layout + LRU eviction), no git needed:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evicts_oldest_over_cap() {
        // A cache with room for 2 bases; touching a third evicts the least-recent.
        let mut idx = CacheIndex::new(2);
        idx.touch("aaa");
        idx.touch("bbb");
        assert_eq!(idx.evict_if_over(), None);     // 2 <= cap
        idx.touch("ccc");                          // now 3 > cap
        assert_eq!(idx.evict_if_over(), Some("aaa".to_string())); // oldest goes
        idx.touch("bbb");                          // refresh bbb
        idx.touch("ddd");                          // 3 > cap again
        assert_eq!(idx.evict_if_over(), Some("ccc".to_string())); // ccc now oldest
    }
}
```

- [ ] **Step 2: Run it → FAIL.** `cargo test -p entheai-federation evicts_oldest`.

- [ ] **Step 3: Implement `base_cache.rs`** — a small LRU index over a cache directory of bare repos, plus a get-or-materialize entry point:

```rust
//! Per-node cache of materialized base repos (one bare repo per base commit).
//! Coders attach detached worktrees off these; the object store is shared. This
//! is an optimization: a miss or any error just means "materialize fresh", never
//! a failure.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// LRU bookkeeping (which base_shas are cached, least-recent first). The bytes on
/// disk are the bare repos under the cache dir; this only tracks order + count.
pub struct CacheIndex {
    cap: usize,
    order: VecDeque<String>, // front = least-recent
}
impl CacheIndex {
    pub fn new(cap: usize) -> Self { Self { cap: cap.max(1), order: VecDeque::new() } }
    /// Mark `sha` most-recently-used (insert or move to back).
    pub fn touch(&mut self, sha: &str) {
        self.order.retain(|s| s != sha);
        self.order.push_back(sha.to_string());
    }
    /// If over capacity, pop + return the least-recent sha to evict (one at a time).
    pub fn evict_if_over(&mut self) -> Option<String> {
        if self.order.len() > self.cap { self.order.pop_front() } else { None }
    }
}

/// The cache directory (per node, persists across tasks for the worker's uptime).
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("entheai-worker-base-cache")
}
fn bare_path(dir: &Path, base_sha: &str) -> PathBuf {
    dir.join(format!("{base_sha}.git"))
}
```

- [ ] **Step 4: Add the `BaseCache` struct** — the LRU index plus a **materialization lock** so concurrent coders that both miss on the same new base don't race on the same directory (the first materializes; the rest wait and then see the hit):

```rust
use crate::Federation;

/// The per-node base cache: an LRU index plus a materialization lock that
/// serializes first-time materialization.
pub struct BaseCache {
    index: tokio::sync::Mutex<CacheIndex>,
    materializing: tokio::sync::Mutex<()>,
}
impl BaseCache {
    pub fn new(cap: usize) -> Self {
        Self { index: tokio::sync::Mutex::new(CacheIndex::new(cap)), materializing: tokio::sync::Mutex::new(()) }
    }

    /// True if this base's bare repo is already on disk (cheap; for the hit/miss tag).
    pub fn bare_exists(&self, base_sha: &str) -> bool { bare_path(&cache_dir(), base_sha).exists() }

    /// A ready shared bare repo for `base_sha`, materialized from the object store
    /// on a miss. Concurrent callers on the same new base serialize on the
    /// materialization lock and the later ones take the (double-checked) hit. On
    /// ANY error the caller falls back to a fresh full clone — never fatal.
    pub async fn get_or_materialize(&self, fed: &Federation, base_sha: &str, base_bundle_key: &str)
        -> anyhow::Result<PathBuf>
    {
        let dir = cache_dir();
        tokio::fs::create_dir_all(&dir).await?;
        let bare = bare_path(&dir, base_sha);

        if self.valid_hit(&bare).await { self.index.lock().await.touch(base_sha); return Ok(bare); }

        // Serialize materialization; re-check under the lock (someone may have just done it).
        let _mg = self.materializing.lock().await;
        if self.valid_hit(&bare).await { self.index.lock().await.touch(base_sha); return Ok(bare); }

        let _ = tokio::fs::remove_dir_all(&bare).await; // clear any partial/broken dir
        let tmp_bundle = dir.join(format!("{base_sha}.bundle"));
        tokio::fs::write(&tmp_bundle, fed.get_bundle(base_bundle_key).await?).await?;
        crate::repo::materialize_bare(&tmp_bundle, &bare).await?;
        let _ = tokio::fs::remove_file(&tmp_bundle).await;

        let evicted = { let mut idx = self.index.lock().await; idx.touch(base_sha); idx.evict_if_over() };
        if let Some(old) = evicted { let _ = tokio::fs::remove_dir_all(bare_path(&dir, &old)).await; }
        Ok(bare)
    }

    async fn valid_hit(&self, bare: &Path) -> bool {
        bare.exists() && crate::repo::rev_parse_ok(bare, "entheai-fed-base").await
    }
}
```

Note the eviction guard: `evict_if_over` removes the least-recent base's *bare repo*. An in-flight coder could still hold a worktree off it — so a base is only a candidate for eviction once it's the least-recent AND over cap; size the cap comfortably above `max_concurrent_coders` so a base in active use is never the eviction target. (The Final review checks this.)

Add the tiny `rev_parse_ok` helper to `repo.rs`:

```rust
/// True if `rev` resolves in `dir` (a quick sanity check for a cached bare repo).
pub async fn rev_parse_ok(dir: &Path, rev: &str) -> bool {
    git(dir, &["rev-parse", "--verify", "--quiet", rev]).await.map(|(ok, _)| ok).unwrap_or(false)
}
```

- [ ] **Step 5: Register + build.** Add `pub mod base_cache;` to `crates/federation/src/lib.rs`. `cargo test -p entheai-federation` (the eviction test passes; the rest still green); `cargo clippy -p entheai-federation`.

- [ ] **Step 6: Commit.** `git add crates/federation && git commit -m "feat(federation): per-node base cache (get-or-materialize + LRU)"`

---

## Task 3: Bounded-concurrency serve loop

**Files:** Modify `bin/entheai-worker/src/main.rs`; `crates/config/src/lib.rs`.

**Context:** The serve loop `await`s one `process_one` at a time (`bin/entheai-worker/src/main.rs:181-212`). Run up to N concurrently under a semaphore: acquire a permit **before** claiming (so we only claim what we can process — no held-but-idle items), then spawn the processing with the permit moved in so it's released on completion.

- [ ] **Step 1: Add the config knob.** In `crates/config/src/lib.rs` `FederationConfig`, add `#[serde(default = "default_max_concurrent")] pub max_concurrent_coders: usize` with `fn default_max_concurrent() -> usize { 4 }`. Add a test that it defaults to 4 and parses. Update the hand-written `Default` impl.

- [ ] **Step 2: Make the shared state spawnable.** In `run_serve`, wrap the coder-run inputs so they can move into a `'static` task: `let config = std::sync::Arc::new(config.clone());` (or thread an `Arc<Config>` in), keep `fed: Federation` (already `Clone`), `config_path: String`, `test_coder: Option<String>`, and an `Arc<Presence>`. Add an in-flight counter for presence: `let inflight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));`.

- [ ] **Step 3: Rewrite the loop for bounded concurrency:**

```rust
let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(
    config.federation.max_concurrent_coders.max(1),
));
let cache = std::sync::Arc::new(
    entheai_federation::base_cache::BaseCache::new(config.federation.base_cache_count()),
);
loop {
    // Only claim when we have capacity to process (never hold idle claimed items).
    let permit = sem.clone().acquire_owned().await.expect("semaphore not closed");
    let claimed = match fed.claim(std::time::Duration::from_secs(20)).await {
        Ok(Some(c)) => c,
        Ok(None) => { drop(permit); continue }
        Err(e) => { log::warn!("claim failed ({e})"); drop(permit); tokio::time::sleep(std::time::Duration::from_secs(2)).await; continue }
    };
    let (fed_c, cfg_c, cfgp_c, tc_c) = (fed.clone(), config.clone(), config_path.clone(), test_coder.clone());
    let (cache_c, presence_c, inflight_c) = (cache.clone(), presence.clone(), inflight.clone());
    tokio::spawn(async move {
        let _permit = permit; // released when this task ends
        inflight_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        presence_c.set(entheai_federation::WorkerState::Working { task: claimed.item.task.clone() });
        let result = process_one(&fed_c, &cfg_c, &cfgp_c, &claimed.item, tc_c.as_deref(), &cache_c)
            .await
            .unwrap_or_else(|e| error_result(&claimed.item, e));
        if inflight_c.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            presence_c.set(entheai_federation::WorkerState::Idle);
        }
        fed_c.publish_result(&result).await.ok();
        claimed.ack().await;
    });
}
```

`error_result(item, e)` is the existing `unwrap_or_else` body, extracted to a small fn. `process_one` gains a `cache: &std::sync::Arc<entheai_federation::base_cache::BaseCache>` parameter (Task 4).

- [ ] **Step 4: Add `base_cache_count()`** to `FederationConfig` (translate `base_cache_mb` or a count knob into an LRU capacity — for the MVP a simple count, default 8): `pub fn base_cache_count(&self) -> usize { 8 }` (or a config field; keep it a constant-backed method for now, documented as tunable).

- [ ] **Step 5: Build + tests.** `cargo build -p entheai-worker`; `cargo test -p entheai-worker -p entheai-config`; `cargo clippy -p entheai-worker`. (Concurrency correctness is verified by the dev-cx53 E2E in Final; here confirm it compiles + the config test passes.)

- [ ] **Step 6: Commit.** `git add bin/entheai-worker crates/config && git commit -m "feat(worker): bounded-concurrency serve loop ([federation] max_concurrent_coders)"`

---

## Task 4: Use the cache + worktree in `process_one`, with fail-fast fallback

**Files:** Modify `bin/entheai-worker/src/main.rs` (`process_one`); `crates/federation/src/lib.rs` (`WorkResult`).

**Context:** Replace the per-coder download + full clone (`main.rs:225-229`) with: get the shared bare repo from the cache, add a detached worktree, run the coder (unchanged), bundle the delta, remove the worktree. On ANY failure of the fast path, fall back to today's full clone and tag the result `degraded`.

- [ ] **Step 1: Add the outcome tag to `WorkResult`.** In `crates/federation/src/lib.rs`, add `#[serde(default)] pub base: String` to `WorkResult` (values `"hit"`, `"miss"`, `"degraded:<reason>"`). Update constructors/tests. This is the loud, structured signal the orchestrator reads.

- [ ] **Step 2: Rewrite the materialize section of `process_one`.** Replace the tempdir download+materialize with a helper that tries the fast path under a deadline and falls back:

```rust
// tmp owns the worktree dir + the fallback clone dir; the shared bare repo lives
// in the persistent cache, NOT under tmp. `bare_used` is Some(path) on the fast
// path (so we can remove the worktree after), None on the fallback clone.
let tmp = tempfile::tempdir()?;
let work = tmp.path().join("work");
let (bare_used, base_outcome): (Option<std::path::PathBuf>, String) = match tokio::time::timeout(
    std::time::Duration::from_secs(20),
    prepare_worktree(fed, item, &work, cache),
).await {
    Ok(Ok((bare, tag))) => (Some(bare), tag),    // tag = "hit" | "miss"
    Ok(Err(e)) => { fallback_full_clone(fed, item, tmp.path(), &work).await?; (None, format!("degraded:{e}")) }
    Err(_)     => { fallback_full_clone(fed, item, tmp.path(), &work).await?; (None, "degraded:timeout".into()) }
};
```

with:

```rust
/// Fast path: cached shared bare repo + a detached worktree at `work`. Returns the
/// shared bare-repo path (so the caller can remove the worktree afterward) and
/// "hit" if the base was already cached, else "miss".
async fn prepare_worktree(
    fed: &entheai_federation::Federation,
    item: &entheai_federation::WorkItem,
    work: &std::path::Path,
    cache: &std::sync::Arc<entheai_federation::base_cache::BaseCache>,
) -> anyhow::Result<(std::path::PathBuf, String)> {
    let existed = cache.bare_exists(&item.base_sha); // cheap pre-check for the tag
    let bare = cache.get_or_materialize(fed, &item.base_sha, &item.base_bundle_key).await?;
    entheai_federation::repo::add_worktree(&bare, work).await?;
    Ok((bare, if existed { "hit".into() } else { "miss".into() }))
}

/// Slow path (today's behavior): download the bundle + full clone into `work`.
async fn fallback_full_clone(
    fed: &entheai_federation::Federation,
    item: &entheai_federation::WorkItem,
    tmp: &std::path::Path,
    work: &std::path::Path,
) -> anyhow::Result<()> {
    let base_bundle = tmp.join("base.bundle");
    tokio::fs::write(&base_bundle, fed.get_bundle(&item.base_bundle_key).await?).await?;
    entheai_federation::repo::materialize_from_bundle(&base_bundle, work).await?;
    Ok(())
}
```

(`cache.bare_exists` is the method defined on `BaseCache` in Task 2.)

- [ ] **Step 3: Remove the worktree after the coder finishes.** After `commit_and_bundle_delta`, detach the shared-base worktree if the fast path was used: `if let Some(bare) = &bare_used { let _ = entheai_federation::repo::remove_worktree(bare, &work).await; }`. Fallback clones live entirely under `tmp` and are dropped with it (nothing shared to detach).

- [ ] **Step 4: Set the tag on the result.** Thread `base_outcome` into the `WorkResult { base: base_outcome, .. }` returned from `process_one` (both success and the coder-error paths). Quiet misses are just data; a `degraded:*` value is the loud signal — also `log::warn!` on `degraded`.

- [ ] **Step 5: Build + smoke.** `cargo build -p entheai-worker`; `cargo test -p entheai-worker`; `cargo clippy`. Local smoke (no NATS needed): unit-test `prepare_worktree` against a temp fake `Federation`? If `Federation` can't be faked cheaply, cover `add_worktree`/`get_or_materialize` via the `repo.rs`/`base_cache.rs` tests (Tasks 1–2) and leave the `process_one` wiring for the dev-cx53 E2E.

- [ ] **Step 6: Commit.** `git add bin/entheai-worker crates/federation && git commit -m "feat(worker): run coders in cached shared-base worktrees, fail-fast to full clone"`

---

## Task 5: Document the knobs + the behavior

**Files:** `entheai.toml`, `docs/entheai-worker.md`, `CHANGELOG.md`.

- [ ] **Step 1: `entheai.toml`** — in `[federation]`, add `max_concurrent_coders = 4  # coders a worker runs at once (they're model-wait-bound)` with a comment that coders share one cached base repo per base commit.
- [ ] **Step 2: `docs/entheai-worker.md`** — a short "Concurrency & the shared base" note: N concurrent coders, each a detached worktree off a per-node cached bare repo (shared object store), fail-fast to a full clone, and the `base = hit|miss|degraded` outcome on each result.
- [ ] **Step 3: `CHANGELOG.md`** — an `[Unreleased]` entry under Performance.
- [ ] **Step 4: Commit.** `git add entheai.toml docs/entheai-worker.md CHANGELOG.md && git commit -m "docs: worker concurrency + shared base cache"`

---

## Final

- [ ] **Full build + test.** `cargo build && cargo build --no-default-features && cargo test --workspace` — all green.
- [ ] **dev-cx53 concurrent E2E (Linux, real).** rsync + build the worker; run `--serve` with `max_concurrent_coders = 3`; dispatch 3 coder tasks on one base (use `--test-coder` for zero LLM cost); confirm: (a) all three complete and their deltas apply to distinct `fed/…` branches, (b) only **one** `*.git` bare repo appears in the cache dir (shared), (c) the results carry `base = miss` then `hit`, (d) killing one mid-run doesn't disturb the others. See memory `dev-cx53-sandbox` for the run recipe.
- [ ] **Fail-fast check.** Point a worker at a bogus `base_bundle_key` (or a corrupt cached repo) and confirm it falls back to a full clone, tags `degraded:*`, and still completes.
- [ ] **Final review** — dispatch a code reviewer over the whole Slice-1 diff (focus: the semaphore never holds unprocessed claimed items; worktrees are always removed; the cache eviction can't delete a bare repo an in-flight coder is using; the fallback path is truly reached on every failure mode).
