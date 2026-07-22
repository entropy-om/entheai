+++
name = "github"
domain = "source control & collaboration"
triggers = ["git", "github", "pr", "pull request", "commit", "branch", "merge", "rebase", "gh ", "actions", "ci"]
rank = 1.0
+++
**Git + GitHub** is the reproducible source of truth for code — the version-control analogue
of what NixOS is for systems.

**Commits:** small, focused, imperative subject ("fix X", "add Y"); one logical change per
commit; never commit secrets (`.env` gitignored). Conventional-commit prefixes
(`feat:`/`fix:`/`docs:`/`ci:`) read well and drive changelogs.

**Branches/PRs:** branch off the default; scoped `git add <paths>` (never `-A`) so unrelated
changes don't ride along; PRs small enough to review in one sitting. A second reviewer (or
an adversarial review agent) before merge catches what the author can't see.

**CI:** GitHub Actions — pin third-party actions by SHA (supply-chain safety), least
`permissions:`, `concurrency:` to cancel stale runs, secrets via `${{ secrets.X }}` only.
Gate merges on build + test + lint (`-D warnings`).

**CLI:** `gh` for PRs/issues/releases/API. Prefer `gh` over hand-rolled API calls.
