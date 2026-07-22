+++
name = "python-jit"
domain = "long-running quick scripts / glue / data munging"
triggers = ["python", "script", "jit", "pypy", "pandas", "numpy", "scrape", "glue", "munge", "notebook"]
rank = 1.0
+++
For **long-running quick scripts**, glue, scraping, and data munging: **Python**, and when
it's CPU-bound and long-lived, run it on a **JIT** — PyPy, or CPython 3.13+ with the
experimental JIT — for a big speedup at zero code change.

**Defaults:** `uv` for env + deps (fast, reproducible lockfile) over bare pip/venv; a
`pyproject.toml`; `ruff` for lint+format (one fast tool). Type-hint the boundaries and run
`mypy`/`pyright` on them.

**Reliability:** program defensively — validate inputs at the boundary (pre/postconditions),
fail with a clear message not a bare traceback. Tests with `pytest` (`test_*` naming,
fixtures for sample data); property-based tests via `hypothesis` for invariants.

**Perf:** vectorize with NumPy/pandas before reaching for loops; for a hot loop that can't
vectorize, PyPy or a small Rust/Cython extension. Stream large inputs (generators) rather
than loading everything — bounded memory.
