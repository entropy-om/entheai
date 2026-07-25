+++
name = "the-fold-and-the-base-case"
domain = "control — loops, retries, repair, termination"
triggers = ["loop", "recursion", "recursive", "retry", "retries", "infinite", "overflow", "repair", "stuck", "spiral", "terminate", "backoff", "max depth", "max_recursion"]
rank = 0.85
+++
Any loop that could run forever must have one of two things, or it is a bug: a **fold** or a **base case**.

- **The base case** is a hard limit — a depth cap, a retry ceiling, a wall-clock budget, a stop rule. `MAX_RECURSION_DEPTH = N`. The recursion terminates because you decided where it stops. A retry without a ceiling, a repair actor without a stop rule, a poll without a timeout — each is a stack overflow waiting to happen.
- **The fold** is graceful bounding — the value wraps instead of shattering. An integer that folds back around max int *is* an oscillator, not a crash. Clamp, saturate, wrap, degrade — bound the range so overflow becomes signal, not death.

The only loop that actually crashes has **neither**. So every retry, poll, repair pass, and recursive descent gets a fold or a base case before it ships — never both absent.

Applied to repair (see `verification` / VRR-Stop): a fallible verifier plus unbounded "just fix it again" can destroy a correct state. Keep the best-verified incumbent; replace only on a real margin; **stop** when there is no marginal value. The stop rule is the base case. Repairing past it is the overflow.
