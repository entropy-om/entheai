# Contributing to entheai

Thanks for your interest in improving entheai.

## Development setup

1. Use macOS on Apple Silicon.
2. Install Rust toolchain `1.96.0` (see `rust-toolchain.toml`).
3. Clone and build:

```bash
git clone https://github.com/entropy-om/entheai.git
cd entheai
cargo build
```

## Before you open a PR

Run the full project checks:

```bash
./scripts/check.sh
```

This runs formatting checks, clippy with warnings denied, and tests.

## Pull request guidelines

1. Keep PRs focused and small when possible.
2. Include a clear problem statement and solution summary.
3. Add or update tests when behavior changes.
4. Update docs when commands, APIs, or behavior change.
5. Link the related issue (if one exists).

## Coding guidelines

1. Follow existing crate boundaries and naming conventions.
2. Prefer small, composable functions and clear trait boundaries.
3. Avoid unrelated refactors in the same PR.
4. Keep public surface changes deliberate and documented.

## Commit messages

Conventional Commits are preferred, for example:

- `feat(core): add tool dispatch retry guard`
- `fix(tools): prevent shell timeout leak`
- `docs: clarify provider config examples`

## Reporting bugs

Please use the bug report issue template and include:

1. Steps to reproduce
2. Expected vs actual behavior
3. Relevant logs or terminal output
4. Environment details (macOS version, Rust version, model/provider)
