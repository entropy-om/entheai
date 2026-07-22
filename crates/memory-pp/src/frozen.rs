//! Frozen nodes — curated best-practice that wakes on deterministic triggers.
//! See docs/superpowers/specs/2026-07-22-frozen-nodes-design.md.

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
}

pub struct FrozenStore {
    nodes: Vec<FrozenNode>,
}

impl FrozenStore {
    pub fn from_nodes(nodes: Vec<FrozenNode>) -> FrozenStore {
        FrozenStore { nodes }
    }

    /// Load every `*.md` in `dir`; skip (warn) any that don't parse. A missing dir
    /// yields an empty store (frozen simply never wakes) — never an error.
    pub fn load(dir: &std::path::Path) -> FrozenStore {
        let mut nodes = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return FrozenStore { nodes };
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            match std::fs::read_to_string(&p).ok().and_then(|raw| FrozenNode::parse(&raw)) {
                Some(n) => nodes.push(n),
                None => log::warn!("frozen: skipping malformed node {}", p.display()),
            }
        }
        FrozenStore { nodes }
    }

    /// Deterministic trigger match → candidates, ordered by lexical relevance of the
    /// prompt to each node's knowledge plus its `rank` prior; best first, ≤ `top_k`.
    pub fn wake(&self, prompt: &str, top_k: usize) -> Vec<FrozenNode> {
        let p = prompt.to_lowercase();
        let mut cands: Vec<&FrozenNode> = self
            .nodes
            .iter()
            .filter(|n| n.triggers.iter().any(|t| trigger_hit(&p, &t.to_lowercase())))
            .collect();
        cands.sort_by(|a, b| {
            let sa = crate::mesh::lexical_score(prompt, &a.knowledge) + 0.25 * a.rank;
            let sb = crate::mesh::lexical_score(prompt, &b.knowledge) + 0.25 * b.rank;
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        cands.into_iter().take(top_k).cloned().collect()
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
        prompt_lc.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(prefix))
    } else {
        prompt_lc.contains(trigger_lc)
    }
}

/// Distil a woken node's knowledge through `mq` (fail-safe: raw on error), cap it, tag it.
/// The returned brief is meant to be injected transiently — NEVER persisted.
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
    format!("❄→☀ frozen:{} — {}", node.name, capped)
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
        assert_eq!(woken[0].name, "nixos", "trigger match + relevance picks nixos");
        assert!(store.wake("unrelated task about cats", 1).is_empty(), "no trigger → no wake");
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
        let brief = activate(&node, &StubMarqant, 4096, std::time::Duration::from_millis(50)).await;
        assert!(brief.contains("frozen:nixos"), "brief is tagged: {brief}");
        assert!(brief.contains("nix flakes"), "brief carries the knowledge");
        // a tiny cap truncates
        let short = activate(&node, &StubMarqant, 12, std::time::Duration::from_millis(50)).await;
        assert!(short.len() <= 64, "respects the byte cap (+ tag): {}", short.len());
    }

    #[test]
    fn loads_real_frozen_dir_nodes() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let frozen_dir = manifest_dir.parent().unwrap().parent().unwrap().join("frozen");
        if frozen_dir.exists() {
            let store = FrozenStore::load(&frozen_dir);
            assert_eq!(store.len(), 11, "real frozen/ dir contains 11 nodes");
        }
    }
}
