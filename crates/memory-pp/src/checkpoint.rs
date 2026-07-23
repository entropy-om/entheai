//! QuantumCheckpoint state engine (roadmap Phase 1.1): 2-way serialization of
//! the transient prompt entropy field. The fluid phase — live frozen-node
//! activations, raw memory span anchors, compression ratio, audio seed —
//! freezes into a rigid JSON checkpoint under `.entheai/checkpoints/<id>.json`,
//! and thaws back without context decay: span ids rehydrate from the raw store
//! (the never-rewritten source of truth), so no payload is ever duplicated
//! into the checkpoint itself.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::PpError;

/// On-disk schema tag. A breaking layout change bumps this AND the crate
/// version (VERSIONING.md wire-format rules).
pub const CHECKPOINT_SCHEMA: &str = "entheai.checkpoint.v1";

/// One frozen node's live activation at snapshot time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrozenActivation {
    pub name: String,
    /// Effective (experience-weighted) rank at snapshot time — the 3.2 overlay
    /// value, not the static front-matter prior.
    pub rank: f32,
}

/// The transient prompt entropy field, frozen to a rigid singularity checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntropyState {
    /// Always [`CHECKPOINT_SCHEMA`]; loads reject anything else.
    pub schema: String,
    pub session_id: String,
    pub created_at_ms: i64,
    /// Active frozen-node activations (name + live rank).
    pub frozen_activations: Vec<FrozenActivation>,
    /// Content ids of the raw spans anchoring this session's context.
    /// Rehydrate via `RawStore::get` — bytes are NOT duplicated here.
    pub raw_span_ids: Vec<String>,
    /// Marqant compression ratio of the last brief (compressed / original),
    /// when the pipeline produced one this session.
    pub marqant_ratio: Option<f32>,
    /// Audio seed state (radio playback seed), when the desktop layer has one.
    pub audio_seed: Option<u64>,
}

impl EntropyState {
    /// Deterministic checkpoint id: blake3 over the serialized state (short
    /// hex). Same field values ⇒ same id — saving twice is idempotent.
    pub fn id(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        let mut h = blake3::Hasher::new();
        h.update(&bytes);
        h.finalize().to_hex()[..16].to_string()
    }

    /// Freeze: write `<dir>/<id>.json` (creating `dir`), returning the id.
    pub fn save(&self, dir: &Path) -> Result<String, PpError> {
        std::fs::create_dir_all(dir)?;
        let id = self.id();
        let path = dir.join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)?;
        Ok(id)
    }

    /// Thaw: read + validate `<dir>/<id>.json`. A wrong or missing schema tag
    /// is an error — never silently reinterpret a foreign file.
    pub fn load(dir: &Path, id: &str) -> Result<EntropyState, PpError> {
        let raw = std::fs::read_to_string(dir.join(format!("{id}.json")))?;
        let state: EntropyState = serde_json::from_str(&raw)?;
        if state.schema != CHECKPOINT_SCHEMA {
            return Err(PpError::Checkpoint(format!(
                "schema mismatch: expected {CHECKPOINT_SCHEMA}, got {}",
                state.schema
            )));
        }
        Ok(state)
    }

    /// Checkpoint ids present under `dir`, newest first by file mtime.
    pub fn list(dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut ids: Vec<(std::time::SystemTime, String)> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let stem = p.file_stem()?.to_str()?.to_string();
                (p.extension()?.to_str()? == "json").then_some(())?;
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((mtime, stem))
            })
            .collect();
        ids.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        ids.into_iter().map(|(_, id)| id).collect()
    }
}

/// Default checkpoint directory relative to a working root.
pub fn default_checkpoint_dir(root: &Path) -> PathBuf {
    root.join(".entheai").join("checkpoints")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> EntropyState {
        EntropyState {
            schema: CHECKPOINT_SCHEMA.to_string(),
            session_id: "sess-1".into(),
            created_at_ms: 1_753_000_000_000,
            frozen_activations: vec![FrozenActivation {
                name: "verification".into(),
                rank: 0.95,
            }],
            raw_span_ids: vec!["blake3:abc".into()],
            marqant_ratio: Some(0.31),
            audio_seed: Some(42),
        }
    }

    #[test]
    fn save_load_round_trips_and_id_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let s = state();
        let id = s.save(dir.path()).unwrap();
        assert_eq!(id, s.id(), "save returns the content id");
        assert_eq!(id, s.id(), "id is deterministic");
        let loaded = EntropyState::load(dir.path(), &id).unwrap();
        assert_eq!(loaded, s, "2-way serialization is lossless");
        // Idempotent: saving the same state lands on the same file.
        assert_eq!(s.save(dir.path()).unwrap(), id);
        assert_eq!(EntropyState::list(dir.path()), vec![id]);
    }

    #[test]
    fn load_rejects_wrong_schema_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = state();
        s.schema = "entheai.checkpoint.v999".into();
        let id = s.id();
        std::fs::write(
            dir.path().join(format!("{id}.json")),
            serde_json::to_vec(&s).unwrap(),
        )
        .unwrap();
        assert!(EntropyState::load(dir.path(), &id).is_err());
        assert!(EntropyState::load(dir.path(), "nope").is_err());
    }
}
