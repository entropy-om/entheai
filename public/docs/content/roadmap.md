---
id: roadmap
title: "Roadmap & Stability"
group: Roadmap
order: 1
---

`entheai` has reached **v1.0.0 Quantum Completeness**, establishing strict public API stability commitments.

## Release History

- **v1.0.0**: Declaration of public API stability contract ([docs/STABILITY.md](file:///Users/peter.lodri/workspace/peterlodri-sec/entheai/docs/STABILITY.md)), deterministic merge seals, byte-reproducible release builds (`scripts/build-repro.sh`), self-auditing recursive dev (`agy`).
- **v1.1.0**: Live current-awareness ingestion (`entheai-current`) via Valyu, WorldMonitor (clamped $\le 50$/day), and daily budget ledgers.
- **v1.2.0**: `karmapa-chenno` call home (`[chenno]`) publishing frozen context briefs to Git; 100 MB shell output caps.
- **v1.3.0**: Full-canvas Zen view (`/zen`), 4 ambient color themes (`/theme`), source-colored current motes, HF `ultrawhale-dogfood` ingestion, and "Mirror in F" procedural audio station.

## Stability Guarantees (SemVer 1.0)

- **Stable**: CLI flags, `entheai.toml` schema, versioned schemas (`entheai.fanout.*`, `entheai.entropy.v1`, `entheai.checkpoint.v1`, `entheai.learning.v1`, `entheai.repro.v1`), verification invariants (`verify_required = true`, SHA-256 seals), and `frozen/*.md` frontmatter format.
- **Unstable**: Internal Rust crate APIs (`entheai-core`, `-orchestrator`, etc.), TUI visual layout details, and tunable experience rank deltas.

<a class="btn btn-ghost" href="https://github.com/entropy-om/entheai/blob/main/CHANGELOG.md" target="_blank" rel="noopener">Full Changelog on GitHub <span>↗</span></a>
