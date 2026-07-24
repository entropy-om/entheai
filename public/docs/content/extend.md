---
id: extend
title: "Skills · plugins · MCP"
group: Concepts
order: 6
---

`entheai` provides three extensibility vectors:

## 1. Skills System (`crates/skills`)

- **Discovery**: Automatically discovers `SKILL.md` instructions in `.agents/skills/` and global customization roots.
- **Web Skill Installer**:
  ```bash
  entheai --skills add https://docs.stripe.com
  ```
  Discovers `.well-known/skills.json` or `/llms.txt`, fetches documentation, and writes `skills/stripe-documentation/SKILL.md`.
- **Management**: `entheai --skills list`, `entheai --skills remove <slug>`.

## 2. Model Context Protocol (`crates/mcp`)

- **stdio Client**: Spawns MCP server binaries at startup and exposes tools to the agent with prefix `<name>__<tool>`.
- **Configured Servers in `entheai.toml`**:
  - `codebase`: `codebase-memory-mcp` (symbol graph).
  - `valyu`: Web & literature search via Python stdio bridge.
  - `smart-tree`: Code intelligence tools via `st --mcp`.
  - `rmcp-sensors`: Environmental sensors.

## 3. Obsidian Sync (`crates/obsidian`)

- Synchronizes session documentation, architecture summaries, and memory logs into an Obsidian vault (`[obsidian] enabled = true`).
