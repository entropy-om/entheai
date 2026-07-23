---
id: tui
title: "Brain ring & swarm graph"
group: "The visual TUI"
order: 1
badgeText: "Visual TUI"
badgeColor: magenta
---

The TUI streams the chat like any terminal agent, plus two always-available live visualizations rendered on a `ratatui` canvas.

**Brain panel** — a rotating faculties graph (model / tools / context) with a footer readout (worker count, NATS up/down, context %, compression ratio). Frozen nodes sit on an outer ring and glow when a task's triggers wake them — either reactively (the prompt matched) or proactively (`BrainJudge` judged recent tool activity relevant, even with no matching words in the prompt itself). The graph's rotation speed reacts to whether you're actually at the keyboard — a direct idle-time poll (the same sensor [`rmcp-sensors`](https://github.com/8bit-wraith/rmcp-sensors)' idle tool wraps) slows it as you step away and brings it back to full speed the moment you return, with a floor so it never fully stops.

**Swarm graph** — appears during `--fanout`: nodes are sub-agents, edges show the fan-out topology, glyphs show per-node status (pending / running / done / failed) as the orchestrator dispatches and merges.

| Command | Action |
|---|---|
| `/brain` | Toggle the brain panel on/off |
| `/config` | Open the config menu — toggle brain panel, swarm graph, fan-out mode, permission mode, and model from one place |
