---
id: arch
title: "Crate map & system"
group: Architecture
order: 1
badgeText: Architecture
---

`entheai` is structured as a modular Rust workspace (resolver v2) compiled for Apple Silicon (`aarch64-apple-darwin`):

```text
Cargo.toml                          # Workspace root
├── bin/entheai/                    # CLI binary (clap, tokio, sentry, mimalloc)
├── bin/entheai-worker/             # Federation worker (--serve / --dispatch)
├── bin/entheai-launch/             # Native .app launcher (Ghostty window)
├── crates/config/                  # TOML -> Config deserialization
├── crates/core/                    # EntheaiAgent (adk-rust backed agent loop)
├── crates/tools/                   # Root-scoped tools (read, write, edit, shell, search)
├── crates/permission/              # Policy & Prompter permission gate
├── crates/router/                  # Role -> model resolution & agent factory
├── crates/orchestrator/            # Fan-out decomposition & worktree pool
├── crates/mapper/                  # @{path} sectioned input mapping
├── crates/tui/                     # Interactive ratatui chat & visualizations
├── crates/companion/               # Session beacon window (QR + status animation)
├── crates/memory/                  # 5-namespace SQLite + vector store
├── crates/memory-pp/               # Prompt-processing (raw store, native 1-bit mesh, Marqant, BrainJudge)
├── crates/current/                 # Current-awareness ingestion (Valyu + WorldMonitor + dogfood)
├── crates/viz/                     # TUI visualization models (brain ring, swarm, Zen field)
├── crates/radio/                   # Audio generator (Standing-Onde + Mirror in F)
├── crates/tts/                     # OS-native text-to-speech engine
├── crates/mcp/                     # MCP client & supervisor
├── crates/skills/                  # SKILL.md discovery & web installer
├── crates/launcher/                # Native app window spawner & shader doctor
├── crates/obsidian/                # Obsidian wiki-sync
├── crates/bus/                     # NATS event bus DTOs & telemetry
├── crates/federation/              # Distributed swarm & work-queue
├── crates/sandbox/                 # Landlock/seccomp worker confinement
├── crates/ultragraph/              # Native Rust ternary 1-bit LLM mesh port
└── crates/kompress-core/           # Context-pruning engine (must-keep overrides)
```

## Binary Distribution & Release Profiles

- **PGO Release (`scripts/build-release.sh`)**: `opt-level=3`, `lto="fat"`, `codegen-units=1`, `target-cpu=native`, `mimalloc` allocator.
- **Reproducible Release (`scripts/build-repro.sh`)**: Anchored `apple-m1` baseline, `SOURCE_DATE_EPOCH`, path remapping, verified byte-identical manifests (`dist/repro-manifest.json`).
