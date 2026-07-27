# entheai in the ecosystem

A map of our repositories across the three orgs, and how each one plugs into
entheai's layers. The point of the map is to make entheai **the tool the rest of
the work routes through** — the provider it infers on, the MCP servers it calls,
the memory it compresses, the swarm it fans out into.

Honest legend — this is where each repo *actually* stands relative to entheai today:

- **●  wired** — reachable from a stock `entheai` right now (in the default config, or a first-class mechanism).
- **○  candidate** — a real repo that fits a layer via an existing mechanism (an MCP server, a provider entry) but isn't in the default wiring yet.
- **◐  adjacent** — sibling/research work that feeds a layer's ideas or models, not a drop-in integration.

The three orgs, in one line each:

| Org | What lives there |
|-----|------------------|
| **entropy-om** | the products — `entheai`, `attestal`/`witness-ai`, the `rivaquant` BitNet models |
| **8b-is** | the lab — tools, MCP servers, kernels, codecs, security utilities (100+ repos) |
| **peterlodri-sec** | the surfaces & infra — `*.vaked.dev`, `osaurus`, `ghostty`, `entheai-vault`, gateways |

---

## Providers / inference — what the router points at

entheai routes `"<provider>/<model>"` ids against `[providers]` (`crates/router`,
`crates/core/model_resolve.rs`). The stock config ships six providers.

| Repo (org) | Layer role | State |
|------------|-----------|-------|
| **coder.vaked.dev** (peterlodri-sec) | The free-tier GPU node — Qwen3-Coder-30B, OpenAI-compatible, **keyless**. Injected as the built-in `[providers.vaked]` and the **fan-out fallback** (`vaked/qwen3-coder:30b`) when nothing else is available. | ● |
| **osaurus** (peterlodri-sec) | Local macOS MLX harness on `127.0.0.1:1337`, OpenAI-compatible — zero-network local inference. | ● |
| deepseek · openrouter · hf · zen | External providers listed in the default config; add a key and switch `default_model`. | ● |
| **MLX-QUANT** (8b-is) | MLX fork with native ternary (BitNet b1.58) CPU kernels — the substrate under fast local ternary inference. | ◐ |
| **rivaquant** / **rivaquant420b** (entropy-om) | From-scratch BitNet b1.58 models — the weights a vaked/osaurus node serves. | ◐ |
| **attestal** / **witness-ai** (entropy-om) | Verified BYOC fine-tuning control plane — produces the models entheai then routes to. | ◐ |
| **portail** (peterlodri-sec) | Unified AI Gateway + MCP Gateway + CDN cache — a single front for many providers. | ○ |

## Tools / MCP layer — external capabilities

Any MCP server declared in `[mcp]` is spawned and its tools registered
(`crates/mcp`, `build_tools`). These are our own servers that belong on that bus.

| Repo (org) | What it gives an agent | State |
|------------|------------------------|-------|
| **bluesky-mcp** (8b-is) | Post/read Bluesky as you + `couchsky_*` public read-only (24 tools). | ○ |
| **smart-tree** (8b-is) | Context-aware, token-frugal directory maps. | ○ |
| **q8-caster** (8b-is) | Display/media casting over MCP. | ○ |
| **RustyNanoKVM** (8b-is) | IP-KVM with an MCP server for AI-driven control. | ○ |
| **int3rceptor · rust-network-scanner · cert-dump · binsider** (8b-is) | Security tooling (intercepting proxy, async scanner, cert extraction, ELF analysis) — natural MCP wrappers for a security-ops agent. | ○ |

## Memory / compression — `crates/memory`, `crates/memory-pp`, `crates/kompress-core`

| Repo (org) | Layer role | State |
|------------|-----------|-------|
| **marqant** (8b-is) | Quantum-compressed markdown — memory-pp shells out to it to compress spans. | ● |
| **headroom** (peterlodri-sec) | Compress tool outputs, logs, and RAG chunks *before* they hit the window — exactly the memory-pp mandate. | ○ |
| **kompress-ultra** (peterlodri-sec) | Compression research feeding `kompress-core`. | ◐ |
| **mem8 · mem8-lite · 8b-Mem8** (8b-is) | Wave-based memory substrate — the ideas behind the memory layer. | ◐ |
| **ultra-graph** (peterlodri-sec) | Pure-Python 1-bit ternary byte-graph LLM — a memory-as-model experiment. | ◐ |

## Skills / knowledge — `crates/skills`, `crates/obsidian`

| Repo (org) | Layer role | State |
|------------|-----------|-------|
| **entheai-vault** (peterlodri-sec) | The Obsidian vault entheai reads through the obsidian layer. | ● |
| skills via `/.well-known/skills.json` | `entheai --skills add <url>` discovers + installs skills from any site. | ● |
| **pocoo.vaked.dev** (peterlodri-sec) | Field notes / build log — knowledge and doctrine, human-readable. | ◐ |

## Federation / orchestration — `crates/orchestrator`, `crates/federation`, `crates/bus`

| Repo (org) | Layer role | State |
|------------|-----------|-------|
| **agy** (Antigravity CLI) | The `[fanout].executor = "agy"` backend — fan-out coders run as recursive `agy` agents. | ● |
| GitHub Copilot CLI | The `[fanout].executor = "copilot"` backend. | ● |
| **ultrameshai** / **ultramesh** (peterlodri-sec) | Decentralized agent routing + lifecycle substrate — where the federation layer wants to dispatch coders. | ○ |
| **moeptimizer** (peterlodri-sec) | Agentic MoE middleware for context optimization — a router in front of the router. | ○ |

## Runtime / shell — `crates/launcher`, `crates/tui`, `crates/companion`

| Repo (org) | Layer role | State |
|------------|-----------|-------|
| **ghostty** (peterlodri-sec fork) | The terminal `entheai --app` opens its dedicated window in (rain-on-glass shader via `--doctor`). | ● |
| **hf-mac** (8b-is) | Native macOS Hugging Face client that embeds entheai as a preview engine. | ◐ |
| **nix-base · vaked-infra** (peterlodri-sec) | Bootstrap flake + provisioning for the nodes entheai runs on and against. | ◐ |

---

*Kept honest on purpose: every repo above is real and every state is where it
actually stands, not where it's aimed. Move a row from ○/◐ to ● only once a stock
`entheai` can reach it — 🜂 ahogy a dolgok vannak.*
