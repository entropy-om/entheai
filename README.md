# entheai

> A personal, macOS-native, **hybrid coding agent for the terminal** — with a brain that fans out.

`entheai` is a coding-agent CLI for Apple Silicon Macs. A strong cloud orchestrator (DeepSeek V4 Pro) plans and decomposes work, then **fans out** to a swarm of sub-agents — each matched to the *best model for its task* — that run in parallel inside isolated git worktrees and merge back only after building and passing tests. It runs local models via [Osaurus](https://github.com/osaurus-ai/osaurus), understands your codebase through a built-in knowledge graph, personalizes to how *you* work, and gets better over time.

Built fresh in **Rust**, taking the best ideas from [Crush](https://github.com/charmbracelet/crush) (UX + YOLO), [CodeWhale](https://github.com/Hmbown/CodeWhale) (durable, sandboxed harness), and [Ruflo](https://github.com/ruvnet/ruflo) (sub-agents, memory, self-learning).

> **Status: early, built in the open.** v0.1 "Foundation" — a streaming agent loop against any OpenAI-compatible provider — is working and merged. The agentic tool loop (read/write/shell/search + permission) is in progress. See [`docs/superpowers/`](docs/superpowers/) for the full design spec and the milestone-by-milestone plans.

## Highlights

- **Tiered hybrid brain** — cloud orchestrator plans; fast local Osaurus workers execute; escalation when it's hard.
- **Fan-out orchestration** — effort-gated decomposition → parallel *model-matched* coders → merge + verify (build & test).
- **Visual by design** — a `ratatui` TUI with **shader backgrounds** (via the Kitty graphics protocol) and a toggleable **live 3D codebase graph**.
- **Memory that compounds** — a five-namespace store (codebase graph, learnings, trajectories, tool results, sub-agent scratch), self-learning routing, and `kompress-v8` automatic context compaction — 100% local, no API keys.
- **Deeply extensible** — native tools · **skills** (superpowers / caveman / BMAD bundled) · **plugins** (managed CLIs) · MCP servers.
- **Knows you** — a built-in [Honcho](https://github.com/plastic-labs/honcho) personalization layer models how you like to work.
- **Self-improving** — a low-overhead flywheel feeds real agent trajectories to a growing dataset.
- **Yours across machines** — federation over your own Tailscale tailnet; a local crash/health sidecar (`Sonar`).
- **macOS / Apple Silicon only** — and it leans all the way into it (Metal, MLX, Seatbelt, terminal graphics).

## Quick start

Requires a recent Rust toolchain and (for local inference) [Osaurus](https://github.com/osaurus-ai/osaurus) running on `127.0.0.1:1337`.

```bash
git clone https://github.com/peterlodri-sec/entheai.git
cd entheai
cargo build --release

# Configure a provider + model (entheai.toml)
cat > entheai.toml <<'TOML'
default_model = "osaurus/qwen3-coder"

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
TOML

# Talk to it
./target/release/entheai "Reply with exactly: pong"
```

Cloud models work too — point a provider at [OpenCode Zen](https://opencode.ai) (DeepSeek V4 Pro/Flash, Qwen, and more through one key):

```toml
default_model = "zen/deepseek-v4-pro"

[providers.zen]
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_API_KEY"
```

Run the checks:

```bash
./scripts/check.sh   # fmt + clippy (-D warnings) + tests
```

## Architecture (v0.1)

A Rust workspace of small, focused crates. The v0.1 foundation is live; later crates are staged in the roadmap.

| Crate | Responsibility |
|---|---|
| `config` | Load TOML settings (providers, models). |
| `providers` | OpenAI-compatible client + `Provider` trait (Osaurus, OpenCode Zen, DeepSeek, OpenRouter). |
| `core` | The agent loop (`Agent` + streaming/tool-dispatch). |
| `entheai` (bin) | The CLI that wires it together. |

Coming next (per the design spec): `router`, `agents` (fan-out), `memory`, `learning`, `tools`, `permission`, `mcp`, `skills`, `plugins`, `session`, `comms`, `tui`, `viz`, `dogfeed`, `compaction`, `honcho`, `sonar`.

## Roadmap

| | |
|---|---|
| **v0.1** | Foundation: streaming agent loop, providers, CLI. ✅ |
| **v0.2** | Agentic tool loop + permission; routing; more tools; durable sessions; visual polish. |
| **v0.3** | Full self-learning; hooks; dogfeed flywheel; Tailscale federation; Sonar UI. |
| **v0.4+** | Pluggable topologies; native 3D graph; Honcho personalization; more providers. |
| **v1.0** | Config freeze, perf passes, docs. |

## Built on

[Osaurus](https://github.com/osaurus-ai/osaurus) · [CodeWhale](https://github.com/Hmbown/CodeWhale) · [Crush](https://github.com/charmbracelet/crush) · [Ruflo](https://github.com/ruvnet/ruflo) · [codebase-memory-mcp](https://github.com/DeusData/codebase-memory-mcp) · [OpenCode Zen](https://opencode.ai) · [Honcho](https://github.com/plastic-labs/honcho) · [Tailscale](https://tailscale.com). Performance practices follow David Lattimore's [*Wild performance tricks*](https://davidlattimore.github.io/posts/2025/09/02/rustforge-wild-performance-tricks.html).

## Thanks to OpenCode 🙏

entheai leans hard on [OpenCode Zen](https://opencode.ai) for cloud inference — DeepSeek V4 Pro/Flash, Qwen, GLM, Kimi, and more through a single OpenAI-compatible key. Genuinely the smoothest model gateway I've used, and the team keeps shipping.

**Try it — you get $5 in credit, they get $5 too → [opencode.ai/go](https://opencode.ai/go?ref=BG9E87CD74)**. Honestly the best referral I've ever seen. Thank you to the whole OpenCode team for all their work. 💛

## License

[Apache-2.0](LICENSE). Note: some bundled or optional components carry their own licenses (e.g. Honcho is AGPL-3.0; Crush is used as design inspiration only, not code) — see the design spec for details.
