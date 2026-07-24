---
id: fanout
title: "Fan-out & sub-agent roles"
navTitle: "Fan-out & roles"
group: Concepts
order: 3
---

When invoked with `entheai --fanout "<task>"`, the orchestrator plans, section-maps the input via `entheai-mapper`, and fans out execution across parallel sub-agents operating in isolated git worktrees (`.worktrees/`).

## Worktree Isolation & Verification Protocol

1. **Parallel Execution**: Each coder sub-agent runs in an isolated git worktree branch (`fed/<session>/<role>`).
2. **Empirical Verification Gate**:
   - `[fanout].verify_required = true` (default).
   - Runs `[fanout].verify` or auto-detected `./scripts/check.sh` inside each coder's worktree.
   - If tests fail, the traceback is auto-ingested into `memory-pp` as a failure trajectory, and the branch is left unmerged.
   - If no verification script resolves, branches remain unmerged on `fed/…` branches for human review (`VerifyStatus::Unverifiable`).
3. **Deterministic `MergeSeal`**:
   - Every verified merge computes `sha256(diff)` and `sha256(verify log)`.
   - Combined seal is logged in the final fan-out report (`integrated ✓ — seal <12-hex>`).

## Recursive Development (`[fanout] executor = "agy"`)

Set `[fanout] executor = "agy"` to run sub-agents via the **Antigravity CLI** (`agy`).

- **Depth Guard**: `ENTHEAI_FANOUT_DEPTH ≤ 3` (hard-capped to prevent infinite loops).
- **Turn Ledger**: Every turn logs to `.entheai/recursion.log` as JSONL.
- **Self-Audit**: Merged diffs are evaluated against `AGENTS.md` rules by an automated post-execution self-audit.
