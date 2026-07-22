+++
name = "verification"
domain = "independent task & subagent verification"
triggers = ["verify", "subagent", "audit", "merge", "worker", "self-reported", "gate", "validation"]
rank = 1.0
+++
Independent verification rule for multi-agent & worker task execution:

**Never trust self-reported success:** Subagents and remote workers can report green completion status while hiding compilation errors, silent test skips, or broken assertions.

**Mandatory Local Execution:** Before accepting or merging subagent outputs into main, execute `./scripts/check.sh` (or explicit `cargo test --workspace` + `cargo clippy --workspace -- -D warnings`) directly in the parent context.

**Empirical Evidence:** Rely strictly on verified exit codes and raw compiler/test runner output logs, never conversational summaries.
