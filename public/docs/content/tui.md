---
id: tui
title: "Visual TUI & Zen field"
group: "The visual TUI"
order: 1
badgeText: "Visual TUI"
badgeColor: magenta
---

The interactive TUI (`crates/tui`) runs on `ratatui` with responsive canvas rendering:

## Core Visual Features

- **Brain Panel (`/brain`)**: A rotating braille 3D pseudo-graph representing model, tools, context, and fleet node presence. Features direct idle-time sensor polling (slowing rotation when user steps away from keyboard) and frozen node wake glows.
- **Swarm Graph**: Inline visualization during `--fanout` runs showing sub-agent worktree nodes, execution state, and empirical verification outcome gold flashes.
- **Zen View (`/zen` or `Ctrl-G`)**: Full-canvas living field with a breathing singularity core (`BrainState::vitality()`), orbiting faculty bodies, frozen constellation ring, current-awareness motes, and dissolving reply motes.
- **Color Themes (`/theme`)**: Switch ambient palettes between `entheia` (teal), `ember` (night fire), `verdant` (garden), and `void` (monochrome + gold thread). Source identity colors (gold, cyan, green) are machine-validated and invariant across themes.
- **Pomodoro Timer**: Automatic 25m work / 5m break countdown displayed in status bar.
- **Modals (`/config` & `/setup`)**: Arrow-key navigable setup wizards for switching models, permission modes, fan-out settings, and themes.

## TUI Command Shortcuts

| Command / Key | Action |
|---|---|
| `/zen` or `Ctrl-G` | Toggle full-canvas Zen field |
| `/theme [name]` | Cycle or set ambient theme (`entheia` / `ember` / `verdant` / `void`) |
| `/brain` | Toggle brain panel side bar |
| `/config` | Open interactive configuration modal |
| `/setup` | Launch first-time setup wizard |
| `Shift+Tab` | Cycle permission posture (`ask` -> `plan` -> `auto` -> `yolo`) |
| `/freeze` / `/thaw` | Snapshot session checkpoint / list and restore checkpoints |
| `/current [pulse]` | Check current-awareness budget status / trigger immediate pulse |
| `/radio` / `Ctrl-P` / `Ctrl-N` | Ambient radio controls (pause / next station) |
| `/speak [on|off|stop]` | Assistant text-to-speech output |
