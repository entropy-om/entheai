# Versioning

`entheai` follows **strict [Semantic Versioning 2.0.0](https://semver.org/)**.

## Single source of truth

Every crate in the workspace shares **one** version, declared once in the root
`Cargo.toml`:

```toml
[workspace.package]
version = "0.1.0"
```

Each crate inherits it with `version.workspace = true`. Never hardcode a
per-crate version — bump the workspace version and all crates move together.

The `entheai-brain` server lives in its own repo and versions independently
under the same rules.

## Bump rules

Given `MAJOR.MINOR.PATCH`:

| Change | Bump | Example |
|---|---|---|
| Breaking change (incompatible API, CLI flag removed/renamed, config schema break, on-disk/wire format break) | **MAJOR** | 1.4.2 → 2.0.0 |
| New backward-compatible functionality (new tool, flag, provider, subcommand) | **MINOR** | 1.4.2 → 1.5.0 |
| Backward-compatible bug fix only | **PATCH** | 1.4.2 → 1.4.3 |

### Pre-1.0 (`0.y.z`) — where we are now

Below 1.0 the public API is not yet stable, so the offsets shift down one place:

- **Breaking change → bump `MINOR`** (`0.1.z` → `0.2.0`).
- **New feature (compatible) → bump `MINOR`** as well (`0.1.z` → `0.2.0`) — in 0.x, features and breaks both move MINOR.
- **Fix only → bump `PATCH`** (`0.1.0` → `0.1.1`).

Reaching a stable, committed public API → release **`1.0.0`**.

## Wire/format compatibility

Two contracts get SemVer discipline of their own — a break in either is a
version bump under the rules above and must be called out in the changelog:

- the **provider/tool** interfaces consumed by sub-agents, and
- the **second-brain wire schema** (learning / trajectory / sync JSON), which is
  itself versioned (`entheai.learning.v1`, …) — a breaking schema change bumps
  both the schema version *and* the crate version.

## Release process

1. Decide the bump (MAJOR/MINOR/PATCH) from the rules above.
2. Update `version` in the root `[workspace.package]`.
3. Update `CHANGELOG.md` (Keep a Changelog format): move `Unreleased` items under
   the new `## [x.y.z] - YYYY-MM-DD` heading.
4. Commit: `chore(release): vX.Y.Z`.
5. Tag: `git tag vX.Y.Z && git push --tags`. The tag drives the release build
   (currently paused — see the disabled `release` workflow; re-enable once the
   GitHub-hosted macOS runner billing is restored, or a self-hosted Apple-Silicon
   runner is added).

## Commit hygiene

Conventional-commit prefixes (`feat:`, `fix:`, `refactor:`, `chore:`, `docs:`)
map cleanly onto the bump rules and make changelog assembly mechanical: `feat` →
MINOR, `fix` → PATCH, a `!` or `BREAKING CHANGE:` footer → MAJOR (MINOR pre-1.0).
