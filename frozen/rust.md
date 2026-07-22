+++
name = "rust"
domain = "systems / backend in Rust"
triggers = ["rust", "cargo", "crate", "clippy", "tokio", "async rust", "rustc", "borrow"]
rank = 1.0
+++
Backend / systems work: reach for **Rust** when correctness + speed both matter.

**Errors:** `thiserror` (typed enums) in *library* crates; `anyhow` (context-rich) in the
*binary*. Never `unwrap()` on external input — bound it and return an error.

**Tests:** one behavior per test, isolated, fast, named for the behavior. Test-first (TDD):
write the failing test, watch it fail for the right reason, minimal code to green. Add
property-based tests (`proptest`) for invariants a handcrafted case would miss.

**Lint gate:** `cargo clippy --all-targets -- -D warnings` in CI — treat pedantic /
complexity / style lints as errors; `cargo fmt` enforced. Clippy's `--explain <CODE>`
tells you *why*. Always independently verify a subagent or worker's self-reported test/clippy results before merging.

**Async:** `tokio`; bound every external call with a timeout + a capped reader; use
`kill_on_drop(true)` on child processes so a timeout can't orphan them.

**Structure:** small, single-responsibility files/modules; a workspace of focused crates
over one giant crate — you (and the compiler) reason better about code that fits in view.
Deterministic, reproducible builds: commit `Cargo.lock`.
