+++
name = "small-repos-for-agents"
domain = "concepts / agentic workflow"
triggers = ["monorepo", "AGENTS.md", "repo scope", "fire a bad session", "context onboarding", "worktree isolation"]
rank = 0.5
+++
A repo's size gates how cheaply you can fire a bad agent session. Starting a
fresh session on a large, many-concern codebase costs real tokens and time
just to orient (read structure, load relevant files, build a mental model)
before any useful work happens — and when that session goes bad (wrong path,
compounding mistakes, confused context), you pay the full onboarding cost
again on the replacement. A small, single-purpose repo with a concise
AGENTS.md hands off to a fresh agent in seconds: it reads a handful of files,
understands scope immediately, gets to work. Monorepos are the sharp end of
this — an agent can wander into an unrelated package, make changes in the
wrong place, or receive contradictory instructions from different parts of
the tree, and the larger the repo, the more expensive getting lost becomes.
The fix isn't "avoid monorepos at all costs," it's: keep each repo/scope
focused on one responsibility, split when different parts need different
context or expertise, and write AGENTS.md to state scope explicitly — what
the repo does, what it doesn't, where the boundaries are. Build for the agent
that's never seen the repo before; it might be starting in five seconds.

Practical carry-over for this codebase: entheai itself *is* a 24-crate
workspace — exactly the shape this practice warns about. The mitigation
already in place is `run_fanout`'s per-coder **git worktree** isolation: each
fan-out sub-agent gets a narrow, single-task checkout rather than the whole
tree, which is the same "small, focused scope per session" benefit this
practice argues for, achieved through worktree isolation instead of separate
repos. Where it's *not* yet mitigated: a fresh session working on the
monorepo directly (not through fan-out) still onboards against the full
`AGENTS.md`/`crates/` map every time. If a session in this repo starts
wandering into unrelated crates or losing track of scope, that's this
pattern manifesting — the fix is scoping the task tighter (a worktree, a
narrower prompt) before assuming the agent itself is at fault.

Source: "Keep repos small so you can fire bad agent sessions fast"
(rwxrob/boost, docs/advanced/), CC BY-NC 4.0 — Robert S. Muhlestein.
