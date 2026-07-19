//! Managed-subtree vault writer (spec §7). FS-direct, atomic, hash-skipped,
//! orphan-GC'd, and confined to `<subtree>/`.

use crate::render::RenderOutput;
use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

const MANIFEST: &str = ".entheai-sync-manifest.json";

/// Writes generated notes/assets into `<vault>/<subtree>/`, tracking a
/// per-path content hash so unchanged notes are not rewritten and vanished
/// sources are garbage-collected.
pub struct VaultWriter {
    subtree: PathBuf,
    /// rel_path (as string) → content hash of the last write.
    manifest: BTreeMap<String, u64>,
    /// rel_paths written during the most recent `apply` (for the nudge).
    last_changed: Vec<PathBuf>,
}

impl VaultWriter {
    /// Open (creating lazily on first write) a writer rooted at `<vault>/<subtree>/`.
    pub fn new(subtree: PathBuf) -> Self {
        let manifest = load_manifest(&subtree);
        Self {
            subtree,
            manifest,
            last_changed: Vec::new(),
        }
    }

    pub fn last_changed(&self) -> &[PathBuf] {
        &self.last_changed
    }

    /// Write the render output: create/update changed notes, copy referenced
    /// assets from `repo_root`, delete orphaned generated files, persist the
    /// manifest. A no-op render (empty output) creates nothing (lazy subtree).
    pub fn apply(&mut self, out: &RenderOutput, repo_root: &Path) -> io::Result<()> {
        self.last_changed.clear();
        if out.is_empty() && self.manifest.is_empty() {
            return Ok(()); // lazy: never materialize an empty subtree
        }

        let mut desired: BTreeMap<String, u64> = BTreeMap::new();

        // Notes — per-item resilient (§8): a single unwritable note (locked file,
        // or an escaping rel_path from a render bug) is logged and skipped, never
        // aborting the pass. A failed write is NOT recorded as current (so it
        // retries next tick); any prior manifest entry is carried forward so a
        // transient failure doesn't orphan-GC the previous good copy.
        for note in &out.notes {
            let key = rel_key(&note.rel_path);
            let hash = fnv1a(note.markdown.as_bytes());
            if self.manifest.get(&key) == Some(&hash) {
                desired.insert(key, hash);
                continue;
            }
            let body = stamp_updated(&note.markdown);
            match self.write_confined(&note.rel_path, body.as_bytes()) {
                Ok(()) => {
                    desired.insert(key, hash);
                    self.last_changed.push(note.rel_path.clone());
                }
                Err(e) => {
                    log::warn!("obsidian: skipping note '{}': {e}", note.rel_path.display());
                    if let Some(&prev) = self.manifest.get(&key) {
                        desired.insert(key, prev);
                    }
                }
            }
        }

        // Assets — copy the file contents from the repo; per-item resilient.
        for asset in &out.assets {
            let key = rel_key(&asset.vault_rel);
            let src = repo_root.join(&asset.repo_rel);
            let bytes = match std::fs::read(&src) {
                Ok(b) => b,
                Err(_) => continue, // missing source asset: skip, not fatal
            };
            let hash = fnv1a(&bytes);
            if self.manifest.get(&key) == Some(&hash) {
                desired.insert(key, hash);
                continue;
            }
            match self.write_confined(&asset.vault_rel, &bytes) {
                Ok(()) => {
                    desired.insert(key, hash);
                    self.last_changed.push(asset.vault_rel.clone());
                }
                Err(e) => {
                    log::warn!(
                        "obsidian: skipping asset '{}': {e}",
                        asset.vault_rel.display()
                    );
                    if let Some(&prev) = self.manifest.get(&key) {
                        desired.insert(key, prev);
                    }
                }
            }
        }

        // Orphan GC: delete manifest entries no longer desired. Resilient — a
        // single failing/poison key (a hand-corrupted "../x" manifest entry, a
        // locked/iCloud-busy file) must NOT wedge the whole sync: log and
        // continue so the manifest still persists and the rest proceeds.
        // Confinement still holds — safe_target rejects an escaping key before
        // any delete happens.
        let orphans: Vec<String> = self
            .manifest
            .keys()
            .filter(|k| !desired.contains_key(*k))
            .cloned()
            .collect();
        for key in orphans {
            if let Err(e) = self.delete_confined(Path::new(&key)) {
                log::warn!("obsidian: skipping orphan '{key}': {e}");
            }
        }

        self.manifest = desired;
        self.persist_manifest()?;
        Ok(())
    }

    /// Join `rel` under the subtree, refusing any path that escapes it.
    ///
    /// Confinement is lexical (component-based); it does not resolve symlinks.
    /// Acceptable under the v1 trust model — entheai owns the subtree and
    /// rel_paths derive from the user's own repo.
    fn safe_target(&self, rel: &Path) -> io::Result<PathBuf> {
        for c in rel.components() {
            match c {
                Component::Normal(_) | Component::CurDir => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("refusing non-confined vault path: {}", rel.display()),
                    ))
                }
            }
        }
        Ok(self.subtree.join(rel))
    }

    fn write_confined(&self, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        let target = self.safe_target(rel)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Atomic: write a temp file in the same dir, then persist (rename).
        let dir = target.parent().unwrap_or(&self.subtree);
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        io::Write::write_all(&mut tmp, bytes)?;
        tmp.persist(&target).map_err(|e| e.error)?;
        Ok(())
    }

    fn delete_confined(&self, rel: &Path) -> io::Result<()> {
        let target = self.safe_target(rel)?;
        match std::fs::remove_file(&target) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn persist_manifest(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.subtree)?;
        let json = serde_json::to_string_pretty(&self.manifest).map_err(io::Error::other)?;
        std::fs::write(self.subtree.join(MANIFEST), json)
    }
}

fn load_manifest(subtree: &Path) -> BTreeMap<String, u64> {
    std::fs::read_to_string(subtree.join(MANIFEST))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn rel_key(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Replace the `{UPDATED}` front-matter token with a fixed synthetic timestamp.
/// NOTE: real wall-clock is injected by the runtime; tests use this stable form.
fn stamp_updated(md: &str) -> String {
    md.replace("{UPDATED}", &now_stamp())
}

/// Current time as unix epoch seconds (kept dep-free; cosmetic metadata).
/// Isolated here so tests can rely on a fixed value via the `test` cfg
/// (avoids nondeterministic hashing — the token is replaced AFTER hashing,
/// so the timestamp never affects change detection).
fn now_stamp() -> String {
    #[cfg(test)]
    {
        "1970-01-01T00:00:00Z".to_string()
    }
    #[cfg(not(test))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("{secs}")
    }
}

/// Stable FNV-1a 64-bit hash (persisted across sessions — must not depend on
/// Rust's per-run `DefaultHasher`).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{AssetRef, VaultNote};

    fn note(rel: &str, md: &str) -> VaultNote {
        VaultNote {
            rel_path: PathBuf::from(rel),
            markdown: md.into(),
        }
    }

    #[test]
    fn writes_note_then_second_identical_apply_is_noop() {
        let vault = tempfile::tempdir().unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());

        let out = RenderOutput {
            notes: vec![note("Home.md", "hi {UPDATED}")],
            assets: vec![],
        };
        w.apply(&out, vault.path()).unwrap();
        assert!(subtree.join("Home.md").is_file());
        assert_eq!(w.last_changed().len(), 1);

        // Re-open + re-apply identical content → no rewrite (hash-skip).
        let mut w2 = VaultWriter::new(subtree.clone());
        w2.apply(&out, vault.path()).unwrap();
        assert!(w2.last_changed().is_empty(), "unchanged note not rewritten");
    }

    #[test]
    fn orphan_note_is_deleted_when_source_vanishes() {
        let vault = tempfile::tempdir().unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());
        w.apply(
            &RenderOutput {
                notes: vec![note("A.md", "a")],
                assets: vec![],
            },
            vault.path(),
        )
        .unwrap();
        assert!(subtree.join("A.md").is_file());
        // Next render no longer includes A.md → GC removes it.
        w.apply(
            &RenderOutput {
                notes: vec![note("B.md", "b")],
                assets: vec![],
            },
            vault.path(),
        )
        .unwrap();
        assert!(!subtree.join("A.md").exists(), "orphan removed");
        assert!(subtree.join("B.md").is_file());
    }

    #[test]
    fn refuses_to_write_outside_subtree() {
        let vault = tempfile::tempdir().unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());
        let out = RenderOutput {
            notes: vec![note("../escape.md", "nope")],
            assets: vec![],
        };
        // Confinement holds: the escaping note is skipped (logged), never written
        // outside the subtree; apply does NOT abort (resilient per-item, §8).
        w.apply(&out, vault.path()).unwrap();
        assert!(
            !vault.path().join("escape.md").exists(),
            "nothing written outside subtree"
        );
        assert!(!subtree.join("escape.md").exists());
    }

    #[test]
    fn one_bad_note_does_not_abort_the_pass() {
        let vault = tempfile::tempdir().unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());
        // A bad (escaping) note must be skipped while the good note is still
        // written and the manifest still persisted.
        let out = RenderOutput {
            notes: vec![note("../escape.md", "nope"), note("Good.md", "ok")],
            assets: vec![],
        };
        w.apply(&out, vault.path()).unwrap();
        assert!(
            subtree.join("Good.md").is_file(),
            "good note written despite the bad one"
        );
        assert!(
            !vault.path().join("escape.md").exists(),
            "confinement still holds"
        );
        assert!(
            subtree.join(".entheai-sync-manifest.json").is_file(),
            "manifest persisted"
        );
    }

    #[test]
    fn copies_referenced_asset_into_subtree() {
        let vault = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("docs/images")).unwrap();
        std::fs::write(repo.path().join("docs/images/brain.png"), b"PNGDATA").unwrap();
        let subtree = vault.path().join("entheai-sync");
        let mut w = VaultWriter::new(subtree.clone());
        let out = RenderOutput {
            notes: vec![],
            assets: vec![AssetRef {
                repo_rel: PathBuf::from("docs/images/brain.png"),
                vault_rel: PathBuf::from("_assets/docs/images/brain.png"),
            }],
        };
        w.apply(&out, repo.path()).unwrap();
        assert_eq!(
            std::fs::read(subtree.join("_assets/docs/images/brain.png")).unwrap(),
            b"PNGDATA"
        );
    }

    #[test]
    fn poison_manifest_key_does_not_wedge_apply_or_escape() {
        let vault = tempfile::tempdir().unwrap();
        let subtree = vault.path().join("entheai-sync");
        std::fs::create_dir_all(&subtree).unwrap();
        // Hand-corrupt the manifest with an escaping key.
        std::fs::write(
            subtree.join(".entheai-sync-manifest.json"),
            r#"{"../escape.md":123}"#,
        )
        .unwrap();
        let mut w = VaultWriter::new(subtree.clone());
        // A normal render must STILL succeed (poison key logged + dropped), and
        // nothing is deleted outside the subtree.
        let out = RenderOutput {
            notes: vec![note("Home.md", "hi")],
            assets: vec![],
        };
        w.apply(&out, vault.path()).unwrap(); // must NOT error
        assert!(subtree.join("Home.md").is_file());
        assert!(
            !vault.path().join("escape.md").exists(),
            "no external delete"
        );
        let m = std::fs::read_to_string(subtree.join(".entheai-sync-manifest.json")).unwrap();
        assert!(
            !m.contains("escape"),
            "poison key dropped from persisted manifest"
        );
    }
}
