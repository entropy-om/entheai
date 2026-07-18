---
name: rust-review
description: Review Rust changes for correctness, idioms, and performance before committing
---
# Rust Review

When reviewing or writing Rust in this project, check:

- **Errors:** prefer `?` over `unwrap()`/`expect()` in non-test code. Library crates use `thiserror`; the binary uses `anyhow`. Never `unwrap()` a lock or a `spawn_blocking` join in library code.
- **Allocation:** avoid needless `clone()`/`to_string()` in hot paths; borrow (`&str`, `&[T]`) where possible; prefer iterators over intermediate `Vec`s.
- **Async:** don't block the runtime — wrap sync/CPU/FS work in `spawn_blocking`; give every network client a timeout.
- **Correctness:** handle the empty/error branches; no silent `let _ =` on fallible results that matter; check integer/`usize` subtraction for underflow.
- **Tests:** does the change have a focused test? Does it still pass `./scripts/check.sh` (fmt + clippy -D warnings + tests)?

Report findings grouped Critical / Important / Minor, most severe first.
