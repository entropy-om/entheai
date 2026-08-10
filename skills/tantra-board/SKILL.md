---
name: tantra-board
description: >
  Control the tantric board at mlxquantlovefrom.com/board — a
  GitHub-issues-backed kanban in peterlodri-sec/mlxquantlovefrom.com with four
  lanes (backlog / burning / tantra / done). Use when the task mentions the
  board, tantra lanes, moving/creating cards, or the tantra-agent CLI. The CLI
  (`tantra-agent list|add|move|todo|summary|whoami|agent`) is the surface; only
  the three collaborators (peterlodri-sec, 8bit-wraith, standardgalactic) have
  write access, each with their own TANTRIC_TOKEN_* env var.
license: MIT
metadata:
  version: "1.0.0"
  author: peterlodri-sec
---

# Tantra board — `tantra-agent`

The tantric board (https://mlxquantlovefrom.com/board) is a kanban backed by
GitHub issues in **`peterlodri-sec/mlxquantlovefrom.com`**. Cards are issues
carrying one lane label: `backlog`, `burning`, `tantra`, `done` (all
pre-existing). Each collaborator's private todo / daily summary is a label-less
issue titled `todo: <login>` / `daily: <login>` (created on first use, appended
to afterwards).

## Tokens — the three collaborators only

Write access belongs to exactly three GitHub accounts, each with their **own**
token in the environment:

| Env var             | Login             |
|---------------------|-------------------|
| `TANTRIC_TOKEN_PETER` | `peterlodri-sec`  |
| `TANTRIC_TOKEN_8BIT`  | `8bit-wraith`     |
| `TANTRIC_TOKEN_SG`    | `standardgalactic` |

Only one of the three is set per environment — that env var IS the active
collaborator (`tantra-agent whoami` reports it). Reads work without a token via
the repo's public read; writes (add / move / todo / summary) require the token.

## CLI

Built in the entheai workspace: `crates/tantra-agent` (run via
`cargo run -p tantra-agent -- ...` from the entheai repo, or install the
binary). The CLI is the primary surface; the adk-rust agent (`agent` subcommand)
is a thin wrapper with `tantra_list` / `tantra_add` / `tantra_move` tools.

```bash
tantra-agent list                                    # all lanes + cards (read)
tantra-agent add --title "JURTA — aug 20" --lane tantra
tantra-agent move --number 3 --lane burning
tantra-agent todo list
tantra-agent todo add "fizetem a starlinket"
tantra-agent summary today "proof of presence kész, felvéve a boardra"
tantra-agent whoami                                   # which collaborator token is active
tantra-agent agent "move card 3 to burning"           # LLM resolves to tantra_move
```

- `list` — groups open issues by lane, then shows `todo:` / `daily:` scratch
  issues. Card format: `#<number> <title>`.
- `add` / `move` — `--lane` accepts `backlog|burning|tantra|done`
  (case-insensitive).
- `todo` / `summary` — per-collaborator, resolved from whichever
  `TANTRIC_TOKEN_*` is set; the `todo: <login>` / `daily: <login>` issue is
  created on first use and appended to with issue comments afterwards.
- Errors are GitHub API status + body; a missing token gets a hint listing the
  three env vars.

## Agent

`tantra-agent agent "<prompt>"` runs an adk-rust `LlmAgent` with the three
tools wired, backed by the free OpenAI-compatible `coder.vaked.dev` node
(override: `TANTRIC_MODEL_URL` / `TANTRIC_MODEL_NAME` / optional
`TANTRIC_MODEL_API_KEY`). Example: "move card 3 to burning" → `tantra_move`.

## Rules for agents touching the board

1. **Only the three collaborators write** — never generate or mint tokens for
   other accounts; if the active `TANTRIC_TOKEN_*` is missing, say so and stop.
2. Prefer the CLI for deterministic ops; use `agent` only when natural-language
   resolution genuinely helps.
3. Lanes are the four labels above — do not invent new labels.
4. Todo/daily issues are per-collaborator scratch: read `todo: <login>` with
   the matching token, never another collaborator's.
