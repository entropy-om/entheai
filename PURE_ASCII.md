# entheai v4.2.1 — Next Chapter

## PURE ASCII SCAFFOLD — e2e spec → proposal

```
+======================================================================+
|                        ENTHEAI v4.2.1 NEXT                            |
|  recursive loops · surface viz · music.vaked.dev · PURE ASCII · TUI   |
+======================================================================+

LAYER 1: Recursive Orchestration (crates/orchestrator)
  - fanout: spawn N subagents, merge results
  - loopback: subagent output -> new subagent input
  - max depth guard: prevent infinite recursion (Spherepop Refuse)
  - state: each subagent has sealed memory (memfd + mlock)

LAYER 2: Surface Visualization (crates/viz)
  - brain ring: neural network topology as rotating graph
  - swarm: particle system showing agent activity
  - matrix: ternary weight matrix {-1, 0, +1} as pixel grid
  - backend: Metal (Apple Silicon) + software fallback

LAYER 3: music.vaked.dev Integration (crates/radio)
  - choreography engine: track sequences define agent state
  - MilkDrop Rust: audio-reactive visualizer
  - quantshuffle: seed-based playlist generation
  - mirror: peer-to-peer shared listening

LAYER 4: PURE ASCII Protocol
  - wire format: 7-bit ASCII commands, UTF-8 payloads
  - commands: /orchestrate /visualize /music /loop /quant
  - response: ASCII-art frames + JSON data blocks
  - terminal: 80x24 minimum, 256-color support

LAYER 5: TUI (crates/tui)
  - ratatui: Rust terminal UI framework
  - panels: chat, viz, music, system, debug
  - keybindings: hjkl navigation, tab switching
  - status: honesty vector hash in status bar

LAYER 6: Graphics + Metal (crates/viz + crates/companion)
  - Metal shader: WebGPU/WGSL compute shaders for matrix viz
  - companion: floating window with live agent state
  - app: native macOS .app bundle
  - launcher: Ghostty terminal + companion window

LAYER 7: Honesty-Auth Integration
  - identity: load ~/.honest-irc/identity.json
  - verification: challenge-response with peers
  - trust: honesty vector hash as agent fingerprint

ROADMAP:
  v4.2.1 (current) — stable agent loop, TUI, memory, MCP
  v4.3.0 — recursive orchestration + loopback
  v4.4.0 — surface viz + Metal shaders
  v4.5.0 — music.vaked.dev choreography
  v4.6.0 — PURE ASCII wire protocol
  v4.7.0 — honesty-auth integration
  v5.0.0 — full mesh: entheai + honest-irc + HarmonyOhm
