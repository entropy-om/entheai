//! Per-node cache of materialized base repos (one bare repo per base commit).
//! Coders attach detached worktrees off these; the object store is shared. This
//! is an optimization: a miss or any error just means "materialize fresh", never
//! a failure.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::Federation;

/// LRU bookkeeping (which base_shas are cached, least-recent first). The bytes on
/// disk are the bare repos under the cache dir; this only tracks order + count.
pub struct CacheIndex {
    cap: usize,
    order: VecDeque<String>, // front = least-recent
}
impl CacheIndex {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: VecDeque::new(),
        }
    }
    /// Mark `sha` most-recently-used (insert or move to back).
    pub fn touch(&mut self, sha: &str) {
        self.order.retain(|s| s != sha);
        self.order.push_back(sha.to_string());
    }
    /// If over capacity, pop + return the least-recent sha to evict (one at a time).
    pub fn evict_if_over(&mut self) -> Option<String> {
        if self.order.len() > self.cap {
            self.order.pop_front()
        } else {
            None
        }
    }
    /// Peek the least-recent sha without removing it (the next eviction candidate).
    pub fn front(&self) -> Option<&str> {
        self.order.front().map(String::as_str)
    }
    /// Remove + return the least-recent sha (front).
    pub fn pop_front_sha(&mut self) -> Option<String> {
        self.order.pop_front()
    }
    /// Number of tracked bases.
    pub fn len(&self) -> usize {
        self.order.len()
    }
    /// True when there are no tracked bases.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    /// True when the tracked count exceeds the capacity.
    pub fn over_cap(&self) -> bool {
        self.order.len() > self.cap
    }
}

/// The cache directory (per node, persists across tasks for the worker's uptime).
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("entheai-worker-base-cache")
}
fn bare_path(dir: &Path, base_sha: &str) -> PathBuf {
    dir.join(format!("{base_sha}.git"))
}

/// The per-node base cache: an LRU index, a materialization lock that serializes
/// first-time materialization, and an in-use refcount so a bare repo a live coder
/// is still attached to is never evicted out from under it.
pub struct BaseCache {
    index: tokio::sync::Mutex<CacheIndex>,
    materializing: tokio::sync::Mutex<()>,
    /// base_sha -> number of coders currently holding a worktree off it. A base
    /// with a non-zero count is pinned: eviction skips it (its shared object store
    /// must not vanish mid-git-operation).
    in_use: tokio::sync::Mutex<std::collections::HashMap<String, usize>>,
}
impl BaseCache {
    pub fn new(cap: usize) -> Self {
        Self {
            index: tokio::sync::Mutex::new(CacheIndex::new(cap)),
            materializing: tokio::sync::Mutex::new(()),
            in_use: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// True if this base's bare repo is already on disk (cheap; for the hit/miss tag).
    pub fn bare_exists(&self, base_sha: &str) -> bool {
        bare_path(&cache_dir(), base_sha).exists()
    }

    /// Mark `base_sha` as in-use by one more coder (pins it against eviction). Every
    /// successful `get_or_materialize` takes exactly one hold; the caller balances
    /// it with one [`release`](Self::release) when the coder's worktree is gone.
    async fn mark_in_use(&self, base_sha: &str) {
        *self
            .in_use
            .lock()
            .await
            .entry(base_sha.to_string())
            .or_insert(0) += 1;
    }

    /// Drop one coder's hold on `base_sha`. At zero holders the key is removed and
    /// the base is eligible for eviction again. Balances one `get_or_materialize`.
    pub async fn release(&self, base_sha: &str) {
        let mut g = self.in_use.lock().await;
        if let Some(n) = g.get_mut(base_sha) {
            *n -= 1;
            if *n == 0 {
                g.remove(base_sha);
            }
        }
    }

    /// If over capacity, pop + return the least-recent base that is NOT in use.
    /// Returns `None` when we're at/under cap OR the least-recent base is pinned
    /// in-use — in that case we tolerate a temporary over-cap rather than delete a
    /// repo a live coder is still attached to. Pure bookkeeping: the caller deletes
    /// the returned base's bare repo from disk. Split out so the in-use guard is
    /// unit-testable without touching the filesystem.
    async fn next_eviction(&self) -> Option<String> {
        let mut idx = self.index.lock().await;
        if !idx.over_cap() {
            return None;
        }
        let front = idx.front()?.to_string();
        if self.in_use.lock().await.contains_key(&front) {
            return None; // least-recent base is pinned; leave the over-cap in place
        }
        idx.pop_front_sha()
    }

    /// Evict least-recent, not-in-use bases from disk until we're at/under cap (or
    /// the least-recent one is pinned in-use). Best-effort deletes.
    async fn evict_over_cap(&self, dir: &Path) {
        while let Some(old) = self.next_eviction().await {
            let _ = tokio::fs::remove_dir_all(bare_path(dir, &old)).await;
        }
    }

    /// A ready shared bare repo for `base_sha`, materialized from the object store
    /// on a miss. Concurrent callers on the same new base serialize on the
    /// materialization lock and the later ones take the (double-checked) hit. On
    /// ANY error the caller falls back to a fresh full clone — never fatal.
    pub async fn get_or_materialize(
        &self,
        fed: &Federation,
        base_sha: &str,
        base_bundle_key: &str,
    ) -> anyhow::Result<PathBuf> {
        let dir = cache_dir();
        tokio::fs::create_dir_all(&dir).await?;
        let bare = bare_path(&dir, base_sha);

        if self.valid_hit(&bare).await {
            self.index.lock().await.touch(base_sha);
            // Pin BEFORE returning: the caller now holds this base until it releases.
            self.mark_in_use(base_sha).await;
            return Ok(bare);
        }

        // Serialize materialization; re-check under the lock (someone may have just done it).
        let _mg = self.materializing.lock().await;
        if self.valid_hit(&bare).await {
            self.index.lock().await.touch(base_sha);
            self.mark_in_use(base_sha).await;
            return Ok(bare);
        }

        let _ = tokio::fs::remove_dir_all(&bare).await; // clear any partial/broken dir
        let tmp_bundle = dir.join(format!("{base_sha}.bundle"));
        tokio::fs::write(&tmp_bundle, fed.get_bundle(base_bundle_key).await?).await?;
        crate::repo::materialize_bare(&tmp_bundle, &bare).await?;
        let _ = tokio::fs::remove_file(&tmp_bundle).await;

        self.index.lock().await.touch(base_sha);
        // Pin the just-materialized base BEFORE evicting, so it can never be chosen
        // as its own eviction victim, and so eviction skips any base still in use.
        self.mark_in_use(base_sha).await;
        self.evict_over_cap(&dir).await;
        Ok(bare)
    }

    async fn valid_hit(&self, bare: &Path) -> bool {
        bare.exists() && crate::repo::rev_parse_ok(bare, "entheai-fed-base").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evicts_oldest_over_cap() {
        let mut idx = CacheIndex::new(2);
        idx.touch("aaa");
        idx.touch("bbb");
        assert_eq!(idx.evict_if_over(), None); // 2 <= cap
        idx.touch("ccc"); // now 3 > cap
        assert_eq!(idx.evict_if_over(), Some("aaa".to_string())); // oldest goes
        idx.touch("bbb"); // refresh bbb
        idx.touch("ddd"); // 3 > cap again
        assert_eq!(idx.evict_if_over(), Some("ccc".to_string())); // ccc now oldest
    }

    #[tokio::test]
    async fn does_not_evict_in_use() {
        // cap 1: "aaa" is pinned in-use, then "aaa" and "bbb" are both tracked (over
        // cap). Even though "aaa" is least-recent, the in-use guard must refuse to
        // evict it — its shared object store is live for a coder's worktree.
        let cache = BaseCache::new(1);
        cache.mark_in_use("aaa").await;
        {
            let mut idx = cache.index.lock().await;
            idx.touch("aaa");
            idx.touch("bbb");
        }
        assert_eq!(
            cache.next_eviction().await,
            None,
            "in-use least-recent base must not be evicted"
        );

        // Once released, "aaa" is eligible again and becomes the eviction target.
        cache.release("aaa").await;
        assert_eq!(
            cache.next_eviction().await,
            Some("aaa".to_string()),
            "released base is evictable"
        );
    }
}
