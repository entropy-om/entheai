---
name: entheai-bridge
description: >
  Hand work to the entheai engine from any coding agent. entheai is a Rust
  agent harness with superpowers OpenCode lacks natively: parallel fan-out
  (isolated worktrees, verification gate, SHA-256 MergeSeal), a NATS worker
  fleet (dispatch tasks to GPU/fleet nodes), offline native ternary inference
  (quantal), skills, and a 5-namespace memory. Use when a task is
  parallelizable (fan-out), belongs on the fleet/GPU (dispatch), needs a
  second opinion from a different model (run), or is a short deterministic
  offline micro-task (quantal).
license: MIT
metadata:
  version: "1.0.0"
  author: peterlodri-sec
---

# entheai-bridge

OpenCode is the **prompting surface**; entheai is the **engine**. This skill
teaches the agent WHEN to hand work over, WHICH entry point, and HOW to read
the result.

## When to hand off

| Situation | Entry point | Why |
|---|---|---|
| Multi-file, parallelizable work with a verify gate | `entheai --fanout "task"` | engine decomposes → parallel coders in worktrees → verify + MergeSeal |
| One task that should run on a fleet/GPU node | `entheai-worker --dispatch --task "…"` | NATS JetStream → worker (vast GPU / dev-cx53) |
| Second opinion from a different model | `entheai --model <p>/<m> "prompt"` | one-shot, answer to stdout |
| Short, deterministic, offline, private micro-task | `entheai --model quantal/<model> "…"` | native ternary, no network — the cogito |
| Query the engine's brain before/after | `entheai --memory search <ns> <query>` | 5 namespaces, read-only |
| Extend the engine's knowledge | `entheai --skills add <url>` | installs into the repo's skills/ |

## The fanout contract (the flagship)

```bash
cd <repo-root>                          # MUST be a git repo; entheai.toml read here
entheai --fanout "the task" --yolo      # --yolo: non-interactive children can't answer stdin
```

Read the report:
- **seal**: the 12-hex prefix is the MergeSeal fingerprint. Full seal = `sha256(diff_sha256 + ":" + log_sha256)`, recomputable: `git diff base..branch | sha256sum`.
- **branches**: integrated → merged into the worktree; unverified/conflicted → left on `fanout/<session>/<n>` for human review.
- **verify**: `./scripts/check.sh` is the auto-detected gate. `VerifyStatus::Unverifiable` → branches stay unmerged, honest.
- **fell-through-local**: if no worker or timeout, it ran locally — say so, don't pretend the fleet did it.

## The dispatch contract (fleet/GPU)

```bash
cd <repo-root>
entheai-worker --dispatch --task "run the benchmark on the GPU"
```
- result: `{ status: committed | no-change | error, branch, log, base: hit|miss|degraded }`
- if `fell_through_local` → the agent runs the task itself. Deadline default 600s.

## The ternary path (quantal — offline cogito)

```bash
entheai --model quantal/<model> "classify this: …"
```
- **Use when**: classification, extraction, reranking, one-line edits, micro-reasoning — short and deterministic.
- **Never**: long generation (128-token default cap, 1024 ceiling), and never fan-out coders (each forward reads ~106 MiB on CPU — a 0.5B model can't carry a coding task).
- The bridge's smartest use: classify a request ("fanout-worthy? fleet-worthy? local?") with quantal BEFORE spending cloud tokens.

## Guardrails

- `cwd` must be the repo root — entheai roots every tool there, reads entheai.toml there, installs skills there.
- **Never pass secrets** (NATS token, API keys) — they come from the server/`.env`.
- Fanout is LONG — don't block on it; poll the job/branches.
- `--yolo` is required for non-interactive runs (default permission mode is `ask` + stdin).
- Skills install is **repo-scoped** — be in the target repo when calling `--skills add`.
- entheai needs a git repo for fanout (worktrees); plain dirs only for run/memory.

## Memory (read-only)

```bash
entheai --memory stats
entheai --memory list <namespace>          # codebase|learnings|trajectories|tools|subagents
entheai --memory search <namespace> <query...>
```
Ask the engine's brain before a run ("has this been tried? what do past trajectories say?") and after ("did it learn anything?"). Writes stay with entheai — if you want it to remember, run it through entheai.
