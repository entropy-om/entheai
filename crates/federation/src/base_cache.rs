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
}

/// The cache directory (per node, persists across tasks for the worker's uptime).
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("entheai-worker-base-cache")
}
fn bare_path(dir: &Path, base_sha: &str) -> PathBuf {
    dir.join(format!("{base_sha}.git"))
}

/// The per-node base cache: an LRU index plus a materialization lock that
/// serializes first-time materialization.
pub struct BaseCache {
    index: tokio::sync::Mutex<CacheIndex>,
    materializing: tokio::sync::Mutex<()>,
}
impl BaseCache {
    pub fn new(cap: usize) -> Self {
        Self {
            index: tokio::sync::Mutex::new(CacheIndex::new(cap)),
            materializing: tokio::sync::Mutex::new(()),
        }
    }

    /// True if this base's bare repo is already on disk (cheap; for the hit/miss tag).
    pub fn bare_exists(&self, base_sha: &str) -> bool {
        bare_path(&cache_dir(), base_sha).exists()
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
            return Ok(bare);
        }

        // Serialize materialization; re-check under the lock (someone may have just done it).
        let _mg = self.materializing.lock().await;
        if self.valid_hit(&bare).await {
            self.index.lock().await.touch(base_sha);
            return Ok(bare);
        }

        let _ = tokio::fs::remove_dir_all(&bare).await; // clear any partial/broken dir
        let tmp_bundle = dir.join(format!("{base_sha}.bundle"));
        tokio::fs::write(&tmp_bundle, fed.get_bundle(base_bundle_key).await?).await?;
        crate::repo::materialize_bare(&tmp_bundle, &bare).await?;
        let _ = tokio::fs::remove_file(&tmp_bundle).await;

        let evicted = {
            let mut idx = self.index.lock().await;
            idx.touch(base_sha);
            idx.evict_if_over()
        };
        if let Some(old) = evicted {
            let _ = tokio::fs::remove_dir_all(bare_path(&dir, &old)).await;
        }
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
}
