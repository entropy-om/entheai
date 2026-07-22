+++
name = "go-parallelism"
domain = "beautiful quick parallelism / concurrent services"
triggers = ["golang", "go ", "goroutine", "channel", "concurrency", "parallel", "errgroup", "worker pool", "waitgroup"]
rank = 1.0
+++
When the job is **quick, concurrent, and wants to read beautifully**, reach for **Go** —
goroutines + channels make parallelism legible.

**Patterns:**
- **Bounded worker pool:** N goroutines ranging over a jobs channel; close the channel to
  signal done; a `sync.WaitGroup` to await workers. Never spawn an unbounded goroutine
  per item.
- **Fan-out / fan-in:** producers → a shared channel → workers → a results channel; a
  collector merges. (This is the graph "parallel fan-out" pattern, natively.)
- **`errgroup.Group`** (`golang.org/x/sync/errgroup`): run parallel subtasks, first error
  cancels the rest via the derived `context`, `g.Wait()` returns it. The default for
  "do these N things concurrently, fail fast."

**Rules:** always pass `context.Context` for cancellation/timeout and honor `ctx.Done()`;
`select` on `ctx.Done()` in every blocking loop. Don't leak goroutines — every one must
have a clear exit. Share memory by communicating (channels), not by locking, when you can.
`go test -race` in CI to catch data races.
