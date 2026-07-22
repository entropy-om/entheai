# frozen/ — curated frozen nodes

Each `*.md` file here is a **frozen node**: a unit of curated best-practice for a problem
space (a preference + its patterns, gotchas, and the right tool/MCP). Nodes sit **dormant**
and **unfreeze** when a task's deterministic triggers match — the node's distilled knowledge
melts into the prompt (bounded, transient) and it glows in the brain panel. See
`docs/superpowers/specs/2026-07-22-frozen-nodes-design.md`.

**The ice-in-coca-cola property:** waking a node doesn't overflow context — only the top
node's distilled brief melts in, capped, then re-freezes when the task passes. The raw node
stays here, always. Determinism over randomness: the *wake* is a deterministic trigger match.

**Not static:** nodes are collected and re-ranked — when a simpler / more deterministic /
reproducible / quick / beautiful way is found for a problem space, update the node.

## File format

```markdown
+++
name = "nixos"                     # unique id
domain = "reproducible cloud …"    # one-line problem space
triggers = ["hetzner", "ssh", …]   # deterministic wake keywords (substring; trailing * = prefix-glob)
mcp = "nixos"                      # OPTIONAL associated MCP (Slice-2 auto-load)
rank = 1.0                         # curated prior; experience-updated later
+++
The distilled best-practice — patterns · gotchas · preferences. Keep it tight; it's what
melts into the prompt.
```

## Current nodes

`nixos` · `terraform` · `docker` · `postgres` · `observability` · `rust` · `go-parallelism`
· `python-jit` · `github` · `ngrok` · `valyu` · `verification`

Seeded from the author's stated preferences, grounded where useful in Valyu deep-research
(e.g. NixOS remote-deploy patterns, Rust lint/test practice). Add more by dropping a new
`<name>.md` here — a malformed file is skipped, never fatal.
