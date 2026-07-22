+++
name = "postgres"
domain = "relational data / durable application state"
triggers = ["postgres", "postgresql", "psql", "sql", "database", "migration", "schema", "index", "pgbouncer"]
rank = 0.9
+++
Default to **PostgreSQL** for durable relational state — battle-tested, correct, feature-deep
(JSONB, full-text, arrays, LISTEN/NOTIFY, partitioning).

**Practices:**
- **Migrations are code:** versioned, forward-only, reviewed, run in CI (sqlx/atlas/flyway).
  Never mutate a schema by hand in prod.
- **Pooling:** a real app pools connections (PgBouncer, or the driver's pool) — Postgres
  backends are heavyweight; don't open a connection per request.
- **Indexing:** index the columns you filter/join/order on; `EXPLAIN (ANALYZE, BUFFERS)`
  before assuming; partial + covering indexes for hot paths; watch for N+1 (batch/join).
- **Correctness:** constraints (`NOT NULL`, `FK`, `CHECK`, `UNIQUE`) in the DB, not just the
  app; transactions for multi-statement invariants; `SELECT … FOR UPDATE` to avoid races.
- **Ops:** parameterized queries only (no string-built SQL — injection); least-priv roles;
  PITR backups tested by actually restoring.

For a local/embedded single-writer store, **SQLite** is the right smaller tool (WAL mode,
one file) — reach for Postgres when you need concurrency, network access, or its depth.
