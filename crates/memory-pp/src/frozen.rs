//! Frozen nodes — curated best-practice that wakes on deterministic triggers.
//! See docs/superpowers/specs/2026-07-22-frozen-nodes-design.md.
//!
//! A frozen node is a named blob of knowledge that is *always* injected as a
//! system message when its trigger words appear in the user prompt. The dyad
//! pair — wake + glow — connects this store to the brain viz panel so every
//! activation renders as a brightening ring node that decays back to dim over
//! subsequent ticks.

use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct FrozenNode {
    pub name: String,
    pub domain: String,
    pub triggers: Vec<String>,
    pub mcp: Option<String>,
    pub rank: f32,
    pub knowledge: String,
}

#[derive(Debug, Deserialize)]
struct FrontMatter {
    name: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    mcp: Option<String>,
    #[serde(default = "default_rank")]
    rank: f32,
}
fn default_rank() -> f32 {
    1.0
}

impl FrozenNode {
    /// Parse a `+++`-fenced TOML front-matter + markdown body. Returns None for a
    /// malformed file (caller skips it) — never panics.
    pub fn parse(raw: &str) -> Option<FrozenNode> {
        let rest = raw.strip_prefix("+++")?;
        let end = rest.find("+++")?;
        let fm: FrontMatter = toml::from_str(rest[..end].trim()).ok()?;
        let knowledge = rest[end + 3..].trim().to_string();
        Some(FrozenNode {
            name: fm.name,
            domain: fm.domain,
            triggers: fm.triggers,
            mcp: fm.mcp,
            rank: fm.rank,
            knowledge,
        })
    }

    /// Human-friendly one-line description for logging.
    pub fn describe(&self) -> String {
        format!(
            "{} [{}] triggers={:?} rank={:.1} knowledge={}b",
            self.name,
            if self.domain.is_empty() {
                "general"
            } else {
                &self.domain
            },
            self.triggers,
            self.rank,
            self.knowledge.len(),
        )
    }
}

pub struct FrozenStore {
    nodes: Vec<FrozenNode>,
    /// Live experience-weighted rank overlay (roadmap Phase 3.2): node name →
    /// effective rank, adjusted by execution outcomes (`rank += delta`) and
    /// consulted by [`FrozenStore::wake`] in place of the static prior.
    /// Seeded from `overlay_path` (JSON `{name: rank}`) when configured.
    ranks: std::sync::RwLock<std::collections::HashMap<String, f32>>,
    /// Best-effort persistence target for the overlay; `None` = in-memory only.
    overlay_path: Option<std::path::PathBuf>,
}

/// Ranks live in `[0, RANK_MAX]`: 0 silences a node's prior entirely; the cap
/// stops a winning-streak node from drowning lexical relevance (wake weights
/// rank at 0.25, so RANK_MAX contributes at most 0.5 to a score).
pub const RANK_MAX: f32 = 2.0;

impl FrozenStore {
    pub fn from_nodes(nodes: Vec<FrozenNode>) -> FrozenStore {
        FrozenStore {
            nodes,
            ranks: std::sync::RwLock::new(std::collections::HashMap::new()),
            overlay_path: None,
        }
    }

    /// Attach a persistent rank overlay: load `{name: rank}` JSON from `path`
    /// (ignoring unknown node names and out-of-range values), and write the
    /// overlay back there, best-effort, on every [`FrozenStore::reweight`].
    pub fn with_overlay(mut self, path: &std::path::Path) -> FrozenStore {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, f32>>(&raw) {
                let known: std::collections::HashSet<&str> =
                    self.nodes.iter().map(|n| n.name.as_str()).collect();
                let filtered: std::collections::HashMap<String, f32> = map
                    .into_iter()
                    .filter(|(k, v)| known.contains(k.as_str()) && (0.0..=RANK_MAX).contains(v))
                    .collect();
                if !filtered.is_empty() {
                    log::info!(
                        "frozen: rank overlay loaded ({} node(s)) from {}",
                        filtered.len(),
                        path.display()
                    );
                }
                *self.ranks.get_mut().expect("fresh lock") = filtered;
            } else {
                log::warn!("frozen: malformed rank overlay {} ignored", path.display());
            }
        }
        self.overlay_path = Some(path.to_path_buf());
        self
    }

    /// A node's live rank: the experience overlay when present, else its
    /// static front-matter prior.
    pub fn effective_rank(&self, node: &FrozenNode) -> f32 {
        self.ranks
            .read()
            .ok()
            .and_then(|m| m.get(&node.name).copied())
            .unwrap_or(node.rank)
    }

    /// Experience-weighted rank update (roadmap Phase 3.2): for every node
    /// whose triggers hit `text`, `rank = clamp(rank + delta, 0, RANK_MAX)`.
    /// Persists the overlay best-effort when configured. Returns the names of
    /// the nodes adjusted (empty = nothing matched; nothing was written).
    pub fn reweight(&self, text: &str, delta: f32) -> Vec<String> {
        let lc = text.to_lowercase();
        let hits: Vec<(String, f32)> = self
            .nodes
            .iter()
            .filter(|n| {
                n.triggers
                    .iter()
                    .any(|t| trigger_hit(&lc, &t.to_lowercase()))
            })
            .map(|n| {
                let new = (self.effective_rank(n) + delta).clamp(0.0, RANK_MAX);
                (n.name.clone(), new)
            })
            .collect();
        if hits.is_empty() {
            return Vec::new();
        }
        let names: Vec<String> = hits.iter().map(|(n, _)| n.clone()).collect();
        if let Ok(mut m) = self.ranks.write() {
            for (name, rank) in hits {
                m.insert(name, rank);
            }
            if let Some(path) = &self.overlay_path {
                match serde_json::to_string_pretty(&*m) {
                    Ok(json) => {
                        if let Err(e) = std::fs::write(path, json) {
                            log::warn!("frozen: rank overlay write failed (continuing): {e}");
                        }
                    }
                    Err(e) => log::warn!("frozen: rank overlay serialize failed: {e}"),
                }
            }
        }
        log::debug!("frozen:reweight {delta:+} → {names:?}");
        names
    }

    /// Load every `*.md` in `dir`; skip (warn) any that don't parse. A missing directory
    /// yields an empty store because frozen nodes are optional domain priors; missing nodes
    /// must gracefully fall back to baseline LLM reasoning rather than crashing agent startup.
    /// Logs a summary of loaded nodes at `info` level on success.
    pub fn load(dir: &std::path::Path) -> FrozenStore {
        let mut nodes = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            log::info!(
                "frozen: no directory at {}, store stays empty",
                dir.display()
            );
            return FrozenStore::from_nodes(nodes);
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            match std::fs::read_to_string(&p)
                .ok()
                .and_then(|raw| FrozenNode::parse(&raw))
            {
                Some(n) => nodes.push(n),
                None => log::warn!("frozen: skipping malformed node {}", p.display()),
            }
        }
        if !nodes.is_empty() {
            log::info!(
                "frozen: loaded {} node(s) from {}",
                nodes.len(),
                dir.display(),
            );
            for n in &nodes {
                log::debug!("frozen:   {}", n.describe());
            }
        }
        FrozenStore::from_nodes(nodes)
    }

    /// Deterministic trigger match → candidates, ordered by lexical relevance of the
    /// prompt to each node's knowledge (primary) plus its `rank` prior (tie-breaker);
    /// best first, ≤ `top_k`. Logs the matched trigger(s) at debug level.
    pub fn wake(&self, prompt: &str, top_k: usize) -> Vec<FrozenNode> {
        let p = prompt.to_lowercase();
        let mut cands: Vec<(&FrozenNode, f32)> = self
            .nodes
            .iter()
            .filter_map(|n| {
                let hits: Vec<&str> = n
                    .triggers
                    .iter()
                    .filter(|t| trigger_hit(&p, &t.to_lowercase()))
                    .map(|s| s.as_str())
                    .collect();
                if hits.is_empty() {
                    None
                } else {
                    let lexical = crate::mesh::lexical_score(prompt, &n.knowledge);
                    // Lexical score is the primary dimension; rank is a fractional
                    // tie-breaker so that among equally-relevant nodes the higher-rank
                    // one wins, but a node with zero term overlap can never outrank one
                    // that actually matches the prompt's vocabulary.
                    Some((n, lexical + 0.25 * self.effective_rank(n)))
                }
            })
            .collect();
        cands.sort_by(|(_, sa), (_, sb)| sb.partial_cmp(sa).unwrap_or(std::cmp::Ordering::Equal));
        let woken: Vec<FrozenNode> = cands
            .into_iter()
            .take(top_k)
            .map(|(n, _)| n.clone())
            .collect();
        if !woken.is_empty() {
            log::debug!(
                "frozen:wake matched {} candidate(s) from prompt {:?}",
                woken.len(),
                truncate_for_log(prompt, 80),
            );
            for n in &woken {
                log::debug!("frozen:wake → {}", n.describe());
            }
        }
        woken
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn nodes(&self) -> &[FrozenNode] {
        &self.nodes
    }
}

/// A trigger matches if it's a substring of the (lowercased) prompt; a trailing `*`
/// makes it a prefix-glob on whitespace-delimited words.
fn trigger_hit(prompt_lc: &str, trigger_lc: &str) -> bool {
    if let Some(prefix) = trigger_lc.strip_suffix('*') {
        prompt_lc
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w.starts_with(prefix))
    } else {
        prompt_lc.contains(trigger_lc)
    }
}

/// Truncate a string for log messages — keeps the first `max` chars, appends `…`
/// if cut. Never panics mid-char.
fn truncate_for_log(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Distil a woken node's knowledge through `mq` (fail-safe: raw on error), cap it, tag it.
/// The returned brief is meant to be injected transiently — NEVER persisted.
/// The tag includes the node's domain when set, giving the model a richer provenance hint.
pub async fn activate(
    node: &FrozenNode,
    marqant: &dyn crate::marqant::Marqant,
    max_bytes: usize,
    deadline: Duration,
) -> String {
    let body = match marqant.compress(&node.knowledge, deadline).await {
        Ok(b) if !b.trim().is_empty() => b,
        _ => node.knowledge.clone(), // mq missing/slow/empty → raw (never blocks)
    };
    let capped = cap_bytes(&body, max_bytes);
    let domain_tag = if node.domain.is_empty() {
        String::new()
    } else {
        format!("@{} ", node.domain)
    };
    format!("❄→☀ frozen:{} {domain_tag}— {}", node.name, capped)
}

fn cap_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_node_reads_frontmatter_and_body() {
        let raw = "+++\nname = \"nixos\"\ndomain = \"cloud\"\ntriggers = [\"hetzner\",\"ssh\"]\nmcp = \"nixos\"\nrank = 1.0\n+++\nPrefer NixOS for deploys.\n";
        let n = FrozenNode::parse(raw).expect("parses");
        assert_eq!(n.name, "nixos");
        assert_eq!(n.triggers, vec!["hetzner", "ssh"]);
        assert_eq!(n.mcp.as_deref(), Some("nixos"));
        assert_eq!(n.rank, 1.0);
        assert_eq!(n.knowledge.trim(), "Prefer NixOS for deploys.");
        // a file without the +++ fences, or with no name, is None (skipped, not a panic)
        assert!(FrozenNode::parse("no frontmatter here").is_none());
    }

    #[test]
    fn store_loads_dir_and_skips_malformed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("nixos.md"),
            "+++\nname=\"nixos\"\ntriggers=[\"hetzner\"]\n+++\nuse nix",
        )
        .unwrap();
        std::fs::write(dir.path().join("broken.md"), "garbage, no frontmatter").unwrap();
        let store = FrozenStore::load(dir.path());
        assert_eq!(store.len(), 1, "malformed file skipped, the good one loads");
        assert_eq!(store.nodes()[0].name, "nixos");
    }

    fn node(name: &str, trigger: &str, rank: f32) -> FrozenNode {
        FrozenNode {
            name: name.into(),
            domain: String::new(),
            triggers: vec![trigger.into()],
            mcp: None,
            rank,
            knowledge: format!("{name} knowledge"),
        }
    }

    #[test]
    fn reweight_adjusts_only_trigger_matched_nodes_and_clamps() {
        let store = FrozenStore::from_nodes(vec![
            node("hetzner", "hetzner", 1.0),
            node("docker", "docker", 1.0),
        ]);
        let touched = store.reweight("deploy to hetzner failed: exit 1", -0.05);
        assert_eq!(touched, vec!["hetzner".to_string()]);
        assert!((store.effective_rank(&store.nodes()[0]) - 0.95).abs() < 1e-6);
        assert_eq!(
            store.effective_rank(&store.nodes()[1]),
            1.0,
            "unmatched untouched"
        );
        // No trigger match → no-op.
        assert!(store.reweight("nothing relevant here", -0.05).is_empty());
        // Clamp floor at 0…
        for _ in 0..40 {
            store.reweight("hetzner", -0.05);
        }
        assert_eq!(store.effective_rank(&store.nodes()[0]), 0.0);
        // …and ceiling at RANK_MAX.
        for _ in 0..100 {
            store.reweight("hetzner", 0.05);
        }
        assert_eq!(store.effective_rank(&store.nodes()[0]), RANK_MAX);
    }

    #[test]
    fn rank_overlay_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let overlay = dir.path().join("frozen-ranks.json");
        let store =
            FrozenStore::from_nodes(vec![node("hetzner", "hetzner", 1.0)]).with_overlay(&overlay);
        store.reweight("hetzner build failed", -0.05);
        assert!(overlay.is_file(), "reweight persists the overlay");

        // A fresh store seeded from the same overlay resumes the learned rank.
        let reloaded =
            FrozenStore::from_nodes(vec![node("hetzner", "hetzner", 1.0)]).with_overlay(&overlay);
        assert!((reloaded.effective_rank(&reloaded.nodes()[0]) - 0.95).abs() < 1e-6);

        // Unknown names and out-of-range values in the overlay are ignored.
        std::fs::write(&overlay, r#"{"ghost": 1.5, "hetzner": 99.0}"#).unwrap();
        let filtered =
            FrozenStore::from_nodes(vec![node("hetzner", "hetzner", 1.0)]).with_overlay(&overlay);
        assert_eq!(
            filtered.effective_rank(&filtered.nodes()[0]),
            1.0,
            "out-of-range overlay value falls back to the static prior"
        );
    }

    #[test]
    fn wake_ordering_follows_the_live_overlay_not_the_static_prior() {
        // Same trigger, same knowledge relevance — rank is the tie-breaker.
        let store =
            FrozenStore::from_nodes(vec![node("a", "deploy", 1.0), node("b", "deploy", 1.0)]);
        // Teach the store that `b` earns wins on this vocabulary.
        for _ in 0..10 {
            store.reweight("deploy", 0.05);
        }
        // Both moved equally so far — now punish `a` alone via a targeted text.
        // (Both share the trigger, so reweight both down then boost b back up.)
        let store =
            FrozenStore::from_nodes(vec![node("a", "deploy", 1.0), node("b", "deploy", 1.5)]);
        let woken = store.wake("deploy the service", 2);
        assert_eq!(woken[0].name, "b", "higher prior wins the tie");
        // Overlay flips the order without touching the static priors.
        store.reweight("deploy", 0.0); // no-op delta, proves plumbing
        let ranks: std::collections::HashMap<String, f32> =
            [("a".to_string(), 2.0), ("b".to_string(), 0.1)].into();
        *store.ranks.write().unwrap() = ranks;
        let woken = store.wake("deploy the service", 2);
        assert_eq!(woken[0].name, "a", "live overlay outranks static prior");
    }

    #[test]
    fn wake_matches_triggers_and_orders_by_relevance() {
        let nodes = vec![
            FrozenNode {
                name: "nixos".into(),
                domain: "cloud".into(),
                triggers: vec!["hetzner".into(), "deploy".into()],
                mcp: None,
                rank: 1.0,
                knowledge: "nixos reproducible deploy to hetzner via ssh".into(),
            },
            FrozenNode {
                name: "ngrok".into(),
                domain: "tunnels".into(),
                triggers: vec!["ngrok".into()],
                mcp: None,
                rank: 1.0,
                knowledge: "ngrok quick tunnel".into(),
            },
        ];
        let store = FrozenStore::from_nodes(nodes);
        let woken = store.wake("please deploy the service to hetzner", 1);
        assert_eq!(woken.len(), 1);
        assert_eq!(
            woken[0].name, "nixos",
            "trigger match + relevance picks nixos"
        );
        assert!(
            store.wake("unrelated task about cats", 1).is_empty(),
            "no trigger → no wake"
        );
    }

    #[tokio::test]
    async fn activate_distills_then_caps() {
        use crate::marqant::StubMarqant;
        let node = FrozenNode {
            name: "nixos".into(),
            domain: "cloud".into(),
            triggers: vec![],
            mcp: None,
            rank: 1.0,
            knowledge: "use nix flakes for pinned inputs".into(),
        };
        // StubMarqant is identity → the brief carries the knowledge, size-capped, tagged.
        let brief = activate(
            &node,
            &StubMarqant,
            4096,
            std::time::Duration::from_millis(50),
        )
        .await;
        assert!(brief.contains("frozen:nixos"), "brief is tagged: {brief}");
        assert!(brief.contains("@cloud"), "domain is tagged: {brief}");
        assert!(brief.contains("nix flakes"), "brief carries the knowledge");
        // a tiny cap truncates
        let short = activate(
            &node,
            &StubMarqant,
            12,
            std::time::Duration::from_millis(50),
        )
        .await;
        assert!(
            short.len() <= 64,
            "respects the byte cap (+ tag): {}",
            short.len()
        );
    }

    #[test]
    fn loads_real_frozen_dir_nodes() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let frozen_dir = manifest_dir
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("frozen");
        if frozen_dir.exists() {
            let store = FrozenStore::load(&frozen_dir);
            assert!(
                store.len() >= 19,
                "real frozen/ dir contains at least 19 nodes"
            );
        }
    }
}
