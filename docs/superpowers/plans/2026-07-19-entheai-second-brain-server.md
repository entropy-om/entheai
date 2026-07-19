# entheai Second-Brain Server — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `entheai-brain` — a self-hosted public API that ingests learnings + trajectories from entheai instances, curates learnings through a corroboration/reputation gate, and serves the promoted collective back for sync into local memory.

**Architecture:** A single Rust **axum** binary over **PostgreSQL 16 + pgvector**. Ingest handlers scrub PII, embed learnings, and route them through a curation engine that clusters semantically-equal learnings and promotes a cluster only after ≥N *distinct* contributors corroborate it. A `GET /v1/sync` cursor serves promoted learnings; a background job mirrors trajectories to a Hugging Face dataset. Auth is a shared bearer token; abuse is bounded by rate limits.

**Tech Stack:** Rust 2021, `axum` 0.7, `tokio`, `sqlx` 0.8 (postgres, rustls), `pgvector` 0.4, `serde`/`serde_json`, `thiserror`, `reqwest` (rustls), `governor` (rate limiting), `regex`, `uuid`; tests use `testcontainers` + `testcontainers-modules` (postgres/pgvector image) and `wiremock`.

**Spec:** `docs/superpowers/specs/2026-07-19-entheai-second-brain-design.md`.

**Repo:** This is a NEW, standalone, Linux-native repo `entheai-brain`, created *outside* the macOS entheai workspace. Create it at `~/workspace/peterlodri-sec/entheai-brain` (a sibling dir). Do NOT add it to the entheai Cargo workspace. This plan doc lives in the entheai repo; all code paths below are relative to the new `entheai-brain/` repo root.

**Prereqs for the implementer:** Docker running (for `testcontainers`); a recent stable Rust toolchain (NOT the entheai `rust-toolchain.toml` pin — this is a separate repo).

## File Structure

```
entheai-brain/
  Cargo.toml
  .env.example
  README.md
  migrations/0001_init.sql        # pgvector ext, 5 tables, indexes
  src/
    main.rs        # bootstrap: config → pool → migrate → router → serve; spawn HF job
    config.rs      # Config::from_env()
    error.rs       # AppError + IntoResponse
    db.rs          # pool + run_migrations
    wire.rs        # JSON contract types (LearningIn, TrajectoryIn, SyncItem, …)
    scrub.rs       # scrub_text / contains_hard_secret
    embed.rs       # Embedder (reqwest → /embeddings)
    curate.rs      # ingest_learning: cluster match → corroborate → promote; reputation; decay
    auth.rs        # RequireBearer extractor
    ratelimit.rs   # governor keyed limiter + layer
    routes.rs      # learnings, trajectories, sync, stats, health handlers + Router
    hf_mirror.rs   # periodic trajectory → HF create_commit job
  deploy/
    entheai-brain.service         # systemd
    Dockerfile
  tests/
    api.rs         # end-to-end endpoint tests against a testcontainers Postgres
```

Each `src/*` file has one responsibility. `curate.rs` is the only nontrivial-logic file; keep handlers thin (parse → call module → respond).

---

### Task 1: Scaffold repo + walking skeleton (health endpoint)

**Files:**
- Create: `entheai-brain/Cargo.toml`, `entheai-brain/src/main.rs`, `entheai-brain/.gitignore`

- [ ] **Step 1: Create the repo + Cargo manifest**

```bash
mkdir -p ~/workspace/peterlodri-sec/entheai-brain/src
cd ~/workspace/peterlodri-sec/entheai-brain
git init
printf 'target/\n.env\n' > .gitignore
```

`Cargo.toml`:
```toml
[package]
name = "entheai-brain"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
anyhow = "1"
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio-rustls", "postgres", "uuid", "chrono", "json", "macros"] }
pgvector = { version = "0.4", features = ["sqlx"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
regex = "1"
governor = "0.6"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[dev-dependencies]
testcontainers = "0.23"
testcontainers-modules = { version = "0.11", features = ["postgres"] }
wiremock = "0.6"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

- [ ] **Step 2: Write a failing test for `GET /health`**

Add to `src/main.rs` a `#[cfg(test)]` module:
```rust
#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    #[tokio::test]
    async fn health_returns_ok() {
        let app = super::health_router();
        let server = axum_test::TestServer::new(app).unwrap();
        let res = server.get("/health").await;
        res.assert_status(StatusCode::OK);
        res.assert_text("ok");
    }
}
```
Add `axum-test = "16"` to `[dev-dependencies]`.

- [ ] **Step 3: Run it — expect FAIL** (`health_router` undefined): `cargo test health_returns_ok`

- [ ] **Step 4: Implement `health_router` + `main`**

`src/main.rs`:
```rust
use axum::{routing::get, Router};

fn health_router() -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 8080));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("entheai-brain listening on {addr}");
    axum::serve(listener, health_router()).await?;
    Ok(())
}
```

- [ ] **Step 5: Run test — expect PASS.** `cargo test health_returns_ok`
- [ ] **Step 6: Commit**
```bash
git add -A && git commit -m "chore: scaffold entheai-brain + /health walking skeleton"
```

---

### Task 2: Config from environment

**Files:** Create `entheai-brain/src/config.rs`, `entheai-brain/.env.example`; Modify `src/main.rs` (add `mod config;`)

- [ ] **Step 1: Failing test** — `src/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn from_map_reads_required_and_defaults() {
        let get = |k: &str| match k {
            "DATABASE_URL" => Some("postgres://x".to_string()),
            "BRAIN_BEARER_TOKEN" => Some("secret".to_string()),
            "EMBED_URL" => Some("http://127.0.0.1:1337/v1".to_string()),
            _ => None,
        };
        let c = Config::from_getter(get).unwrap();
        assert_eq!(c.bearer_token, "secret");
        assert_eq!(c.n_promote, 3);          // default
        assert_eq!(c.tau_match, 0.85);        // default
        assert!(c.fleet_contributor_ids.is_empty());
    }
    #[test]
    fn missing_required_errors() {
        assert!(Config::from_getter(|_| None).is_err());
    }
}
```

- [ ] **Step 2: Run — expect FAIL.** `cargo test config`
- [ ] **Step 3: Implement `src/config.rs`:**
```rust
use std::collections::HashSet;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bearer_token: String,
    pub embed_url: String,
    pub hf_token: Option<String>,
    pub hf_repo: Option<String>,
    pub fleet_contributor_ids: HashSet<String>,
    pub n_promote: i64,
    pub tau_match: f32,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_getter(|k| std::env::var(k).ok())
    }
    pub fn from_getter(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let req = |k: &str| get(k).ok_or_else(|| anyhow::anyhow!("missing env {k}"));
        Ok(Self {
            database_url: req("DATABASE_URL")?,
            bearer_token: req("BRAIN_BEARER_TOKEN")?,
            embed_url: req("EMBED_URL")?,
            hf_token: get("HF_TOKEN"),
            hf_repo: get("HF_REPO"),
            fleet_contributor_ids: get("FLEET_CONTRIBUTOR_IDS")
                .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
                .unwrap_or_default(),
            n_promote: get("N_PROMOTE").and_then(|s| s.parse().ok()).unwrap_or(3),
            tau_match: get("TAU_MATCH").and_then(|s| s.parse().ok()).unwrap_or(0.85),
            port: get("PORT").and_then(|s| s.parse().ok()).unwrap_or(8080),
        })
    }
}
```
Add `mod config;` to `main.rs`; use `Config::from_env()?` + `config.port` in `main`.

`.env.example`:
```
DATABASE_URL=postgres://brain:brain@localhost:5432/brain
BRAIN_BEARER_TOKEN=change-me
EMBED_URL=http://127.0.0.1:1337/v1
HF_TOKEN=
HF_REPO=PeetPedro/ultrawhale-dogfood
FLEET_CONTRIBUTOR_IDS=
N_PROMOTE=3
TAU_MATCH=0.85
PORT=8080
```

- [ ] **Step 4: Run — expect PASS.** `cargo test config`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: env config"`

---

### Task 3: Database schema + migrations + pool

**Files:** Create `entheai-brain/migrations/0001_init.sql`, `entheai-brain/src/db.rs`, `entheai-brain/tests/api.rs` (test harness start); Modify `main.rs` (`mod db;`)

- [ ] **Step 1: Write `migrations/0001_init.sql`:**
```sql
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE contributors (
    contributor_id   TEXT PRIMARY KEY,
    reputation       REAL NOT NULL DEFAULT 1.0,
    is_fleet         BOOLEAN NOT NULL DEFAULT FALSE,
    promoted_count   INTEGER NOT NULL DEFAULT 0,
    contradicted_count INTEGER NOT NULL DEFAULT 0,
    first_seen       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TYPE cluster_status AS ENUM ('pending', 'promoted', 'contradicted');

CREATE TABLE learning_clusters (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    canonical_text       TEXT NOT NULL,
    tags                 TEXT[] NOT NULL DEFAULT '{}',
    embedding            vector(1024) NOT NULL,
    distinct_contributors INTEGER NOT NULL DEFAULT 0,
    confidence           REAL NOT NULL DEFAULT 0,
    status               cluster_status NOT NULL DEFAULT 'pending',
    promotion_seq        BIGINT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX clusters_embedding_hnsw ON learning_clusters USING hnsw (embedding vector_cosine_ops);
CREATE INDEX clusters_promotion_seq ON learning_clusters (promotion_seq) WHERE status = 'promoted';
CREATE SEQUENCE promotion_seq_gen;

CREATE TABLE cluster_members (
    cluster_id      UUID NOT NULL REFERENCES learning_clusters(id) ON DELETE CASCADE,
    contributor_id  TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    outcome         TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (cluster_id, contributor_id)
);

CREATE TABLE trajectories (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    contributor_id  TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    payload         JSONB NOT NULL,
    exported_to_hf  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX trajectories_unexported ON trajectories (created_at) WHERE exported_to_hf = FALSE;
```
NOTE: embedding dim is **1024** — set to match your embedding model; change here + in `wire`/`embed` if different.

- [ ] **Step 2: Implement `src/db.rs`:**
```rust
use sqlx::postgres::{PgPool, PgPoolOptions};

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new().max_connections(16).connect(database_url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
```
Add `mod db;` to `main.rs`; in `main`, `let pool = db::connect(&config.database_url).await?;` (pass into the router later).

- [ ] **Step 3: Write the shared test harness** — `tests/api.rs`:
```rust
use testcontainers_modules::{postgres::Postgres, testcontainers::runners::AsyncRunner};

/// Boots a pgvector Postgres, returns (container, database_url). Keep the container alive.
pub async fn test_db() -> (impl std::any::Any, String) {
    let node = Postgres::default()
        .with_tag("pg16")                // use a pgvector image
        .with_name("pgvector/pgvector")
        .start().await.unwrap();
    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    (node, url)
}

#[tokio::test]
async fn migrations_apply_cleanly() {
    let (_c, url) = test_db().await;
    let pool = entheai_brain::db::connect(&url).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM contributors").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
}
```
To make `entheai_brain::db` visible to `tests/`, add a `src/lib.rs` that re-exports the modules (`pub mod config; pub mod db; pub mod wire; …`) and have `main.rs` `use entheai_brain::*;`. Do this refactor now (create `src/lib.rs`, thin `main.rs`).

- [ ] **Step 4: Run — expect PASS** (Docker required): `cargo test --test api migrations_apply_cleanly`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: postgres schema + migrations + pool + test harness"`

---

### Task 4: Wire contract types

**Files:** Create `entheai-brain/src/wire.rs`; Modify `src/lib.rs` (`pub mod wire;`)

- [ ] **Step 1: Failing test** — `src/wire.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn learning_batch_deserializes() {
        let j = r#"{"contributor_id":"c1","session_id":"s1",
            "learnings":[{"text":"cargo test caught it","tags":["rust"],"outcome":"succeeded"}]}"#;
        let b: LearningBatch = serde_json::from_str(j).unwrap();
        assert_eq!(b.contributor_id, "c1");
        assert_eq!(b.learnings.len(), 1);
        assert_eq!(b.learnings[0].outcome.as_deref(), Some("succeeded"));
    }
    #[test]
    fn sync_item_serializes() {
        let s = SyncItem { id: "id".into(), text: "t".into(), tags: vec![], confidence: 0.9, corroborations: 3 };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["corroborations"], 3);
    }
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test wire`
- [ ] **Step 3: Implement `src/wire.rs`:**
```rust
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct LearningBatch {
    pub contributor_id: String,
    pub session_id: String,
    pub learnings: Vec<LearningIn>,
}
#[derive(Deserialize)]
pub struct LearningIn {
    pub text: String,
    #[serde(default)] pub tags: Vec<String>,
    #[serde(default)] pub confidence: Option<f32>,
    #[serde(default)] pub tool: Option<String>,
    #[serde(default)] pub outcome: Option<String>,
}

#[derive(Deserialize)]
pub struct TrajectoryBatch {
    pub contributor_id: String,
    pub session_id: String,
    pub trajectories: Vec<serde_json::Value>, // opaque §5.18 records
}

#[derive(Serialize)]
pub struct SyncItem {
    pub id: String,
    pub text: String,
    pub tags: Vec<String>,
    pub confidence: f32,
    pub corroborations: i32,
}
#[derive(Serialize)]
pub struct SyncResponse { pub cursor: i64, pub learnings: Vec<SyncItem> }

#[derive(Serialize)]
pub struct Accepted { pub accepted: usize }
```

- [ ] **Step 4: Run — PASS.** `cargo test wire`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: JSON wire contract types"`

---

### Task 5: Error type → HTTP responses

**Files:** Create `entheai-brain/src/error.rs`; Modify `src/lib.rs` (`pub mod error;`)

- [ ] **Step 1: Failing test** — `src/error.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::http::StatusCode;
    #[test]
    fn unauthorized_maps_to_401() {
        assert_eq!(AppError::Unauthorized.into_response().status(), StatusCode::UNAUTHORIZED);
    }
    #[test]
    fn rejected_maps_to_422() {
        assert_eq!(AppError::Rejected("pii".into()).into_response().status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test error`
- [ ] **Step 3: Implement `src/error.rs`:**
```rust
use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unauthorized")] Unauthorized,
    #[error("rate limited")] RateLimited,
    #[error("rejected: {0}")] Rejected(String),
    #[error("bad request: {0}")] BadRequest(String),
    #[error(transparent)] Db(#[from] sqlx::Error),
    #[error(transparent)] Other(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (code, msg) = match &self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, self.to_string()),
            AppError::Rejected(_) => (StatusCode::UNPROCESSABLE_ENTITY, self.to_string()),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::Db(_) | AppError::Other(_) => {
                tracing::error!("internal: {self:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
            }
        };
        (code, Json(json!({"error": msg}))).into_response()
    }
}
```

- [ ] **Step 4: Run — PASS.** `cargo test error`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: AppError + HTTP mapping"`

---

### Task 6: Bearer-token auth extractor

**Files:** Create `entheai-brain/src/auth.rs`; Modify `src/lib.rs`

- [ ] **Step 1: Failing test** — `src/auth.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_token() {
        assert!(token_ok("Bearer secret", "secret"));
        assert!(!token_ok("Bearer wrong", "secret"));
        assert!(!token_ok("secret", "secret"));       // missing "Bearer "
        assert!(!token_ok("", "secret"));
    }
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test auth`
- [ ] **Step 3: Implement `src/auth.rs`** (pure check + an axum middleware fn):
```rust
use axum::{extract::State, http::Request, middleware::Next, response::Response};
use crate::{error::AppError, AppState};

pub fn token_ok(header: &str, expected: &str) -> bool {
    header.strip_prefix("Bearer ").map(|t| constant_eq(t, expected)).unwrap_or(false)
}
fn constant_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() { return false; }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub async fn require_bearer(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, AppError> {
    let h = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    if token_ok(h, &state.config.bearer_token) { Ok(next.run(req).await) }
    else { Err(AppError::Unauthorized) }
}
```
Define `AppState` in `src/lib.rs`: `#[derive(Clone)] pub struct AppState { pub pool: sqlx::PgPool, pub config: std::sync::Arc<config::Config>, pub embedder: std::sync::Arc<embed::Embedder> }` (add `embed` in Task 8; stub the field type now or introduce `AppState` incrementally — introduce `pool` + `config` here, add `embedder` in Task 8).

- [ ] **Step 4: Run — PASS.** `cargo test auth`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: bearer auth (constant-time) middleware"`

---

### Task 7: PII scrub

**Files:** Create `entheai-brain/src/scrub.rs`; Modify `src/lib.rs`

- [ ] **Step 1: Failing tests** — `src/scrub.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_email_ip_phone_home() {
        let out = scrub_text("mail a@b.com from 10.0.0.1 call 415-555-1212 in /Users/pete/x");
        assert!(!out.contains("a@b.com"));
        assert!(!out.contains("10.0.0.1"));
        assert!(!out.contains("415-555-1212"));
        assert!(!out.contains("/Users/pete"));
        assert!(out.contains("[email]") && out.contains("[ip]"));
    }
    #[test]
    fn detects_hard_secret() {
        assert!(contains_hard_secret("key sk-ABCDEFGHIJKLMNOPQRSTUVWX1234567890abcd"));
        assert!(!contains_hard_secret("nothing secret here"));
    }
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test scrub`
- [ ] **Step 3: Implement `src/scrub.rs`** with lazily-built regexes (use `std::sync::LazyLock`):
```rust
use regex::Regex;
use std::sync::LazyLock;

static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\w.+-]+@[\w-]+\.[\w.-]+").unwrap());
static IPV4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{1,3}(?:\.\d{1,3}){3}\b").unwrap());
static PHONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{3}[-.\s]?\d{3}[-.\s]?\d{4}\b").unwrap());
static HOME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(?:Users|home)/[\w.-]+").unwrap());
static SECRET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:sk|ghp|hf|xoxb)[-_][A-Za-z0-9]{20,}\b").unwrap());

pub fn scrub_text(s: &str) -> String {
    let s = EMAIL.replace_all(s, "[email]");
    let s = IPV4.replace_all(&s, "[ip]");
    let s = PHONE.replace_all(&s, "[phone]");
    let s = HOME.replace_all(&s, "[path]");
    s.into_owned()
}
pub fn contains_hard_secret(s: &str) -> bool { SECRET.is_match(s) }
```
NOTE: Rust 1.80+ for `LazyLock`. Scrub is applied to learning text; trajectory payloads are scrubbed by recursively scrubbing string values (add `scrub_json(&mut serde_json::Value)` — test it: a JSON string field with an email is redacted).

- [ ] **Step 4: Run — PASS.** `cargo test scrub`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: server-side PII scrub + hard-secret reject"`

---

### Task 8: Embedding client

**Files:** Create `entheai-brain/src/embed.rs`; Modify `src/lib.rs`, `AppState` (add `embedder`)

- [ ] **Step 1: Failing test (wiremock)** — `src/embed.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};
    #[tokio::test]
    async fn embeds_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "embedding": vec![0.1_f32; 1024] }]
            }))).mount(&server).await;
        let e = Embedder::new(server.uri());
        let v = e.embed("hello").await.unwrap();
        assert_eq!(v.len(), 1024);
    }
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test embed`
- [ ] **Step 3: Implement `src/embed.rs`:**
```rust
pub struct Embedder { client: reqwest::Client, base_url: String }
impl Embedder {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().unwrap(),
            base_url: base_url.into(),
        }
    }
    pub async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({ "input": text, "model": "text-embedding" });
        let v: serde_json::Value = self.client.post(url).json(&body).send().await?.error_for_status()?.json().await?;
        let arr = v["data"][0]["embedding"].as_array().ok_or_else(|| anyhow::anyhow!("no embedding"))?;
        Ok(arr.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
    }
}
```

- [ ] **Step 4: Run — PASS.** `cargo test embed`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: OpenAI-compatible embedding client"`

---

### Task 9: Curation — cluster match + corroborate (raw ingest)

**Files:** Create `entheai-brain/src/curate.rs`; Modify `tests/api.rs`

This is the heart. `ingest_learning` embeds a learning, finds the nearest cluster within `τ_match`; on match from a *new* contributor it corroborates (adds a `cluster_members` row, bumps `distinct_contributors`); otherwise it creates a new `pending` cluster. Promotion is Task 10.

- [ ] **Step 1: Integration test** — add to `tests/api.rs`:
```rust
#[tokio::test]
async fn two_distinct_contributors_corroborate_one_cluster() {
    let (_c, url) = test_db().await;
    let pool = entheai_brain::db::connect(&url).await.unwrap();
    // Deterministic fake embedding: both learnings map to the same vector.
    let emb = vec![0.2_f32; 1024];
    let cfg = test_config();  // helper returning a Config with n_promote=3, tau_match=0.85
    entheai_brain::curate::ingest_learning(&pool, &cfg, "c1", "s1",
        &vecish("cargo test after edits", &emb), None).await.unwrap();
    entheai_brain::curate::ingest_learning(&pool, &cfg, "c2", "s2",
        &vecish("run cargo test post-edit", &emb), None).await.unwrap();
    let clusters: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_clusters").fetch_one(&pool).await.unwrap();
    assert_eq!(clusters, 1, "semantically-equal learnings should share a cluster");
    let dc: i32 = sqlx::query_scalar("SELECT distinct_contributors FROM learning_clusters LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(dc, 2);
}
```
Introduce a `LearningEmbedded { text, tags, embedding }` struct passed to `ingest_learning` so tests inject deterministic embeddings (production path embeds via `Embedder` in the handler, Task 12). `vecish(text, emb)` builds one.

- [ ] **Step 2: Run — FAIL.** `cargo test --test api two_distinct`
- [ ] **Step 3: Implement `src/curate.rs` `ingest_learning`:**
```rust
use pgvector::Vector;
use sqlx::PgPool;
use crate::config::Config;

pub struct LearningEmbedded { pub text: String, pub tags: Vec<String>, pub embedding: Vec<f32> }

pub async fn ingest_learning(
    pool: &PgPool, cfg: &Config, contributor_id: &str, session_id: &str,
    l: &LearningEmbedded, outcome: Option<&str>,
) -> anyhow::Result<()> {
    upsert_contributor(pool, cfg, contributor_id).await?;
    let vec = Vector::from(l.embedding.clone());
    // Nearest cluster by cosine distance; pgvector `<=>` is cosine distance (0..2), sim = 1 - dist.
    let nearest: Option<(uuid::Uuid, f64)> = sqlx::query_as(
        "SELECT id, (embedding <=> $1) AS dist FROM learning_clusters ORDER BY embedding <=> $1 LIMIT 1")
        .bind(&vec).fetch_optional(pool).await?;
    let matched = nearest.filter(|(_, dist)| (1.0 - dist) as f32 >= cfg.tau_match).map(|(id, _)| id);

    if let Some(cluster_id) = matched {
        // Insert member; unique (cluster_id, contributor_id) makes re-corroboration a no-op.
        let inserted = sqlx::query(
            "INSERT INTO cluster_members (cluster_id, contributor_id, session_id, outcome)
             VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(cluster_id).bind(contributor_id).bind(session_id).bind(outcome)
            .execute(pool).await?;
        if inserted.rows_affected() == 1 {
            sqlx::query("UPDATE learning_clusters SET distinct_contributors = distinct_contributors + 1, updated_at = now() WHERE id = $1")
                .bind(cluster_id).execute(pool).await?;
        }
    } else {
        let cluster_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO learning_clusters (canonical_text, tags, embedding, distinct_contributors, status)
             VALUES ($1,$2,$3,1,'pending') RETURNING id")
            .bind(&l.text).bind(&l.tags).bind(&vec).fetch_one(pool).await?;
        sqlx::query("INSERT INTO cluster_members (cluster_id, contributor_id, session_id, outcome) VALUES ($1,$2,$3,$4)")
            .bind(cluster_id).bind(contributor_id).bind(session_id).bind(outcome).execute(pool).await?;
    }
    Ok(())
}

async fn upsert_contributor(pool: &PgPool, cfg: &Config, id: &str) -> anyhow::Result<()> {
    let is_fleet = cfg.fleet_contributor_ids.contains(id);
    sqlx::query(
        "INSERT INTO contributors (contributor_id, is_fleet, reputation) VALUES ($1,$2,$3)
         ON CONFLICT (contributor_id) DO UPDATE SET last_seen = now()")
        .bind(id).bind(is_fleet).bind(if is_fleet { 5.0_f32 } else { 1.0_f32 })
        .execute(pool).await?;
    Ok(())
}
```

- [ ] **Step 4: Run — PASS.** `cargo test --test api two_distinct`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat(curate): cluster match + distinct-contributor corroboration"`

---

### Task 10: Curation — promotion gate

**Files:** Modify `entheai-brain/src/curate.rs`, `tests/api.rs`

- [ ] **Step 1: Integration tests** — add to `tests/api.rs`:
```rust
#[tokio::test]
async fn single_contributor_never_promotes() {
    let (_c, url) = test_db().await; let pool = entheai_brain::db::connect(&url).await.unwrap();
    let cfg = test_config(); let emb = vec![0.3_f32; 1024];
    for _ in 0..5 {  // same contributor, 5 times
        entheai_brain::curate::ingest_learning(&pool, &cfg, "solo", "sX", &vecish("x", &emb), None).await.unwrap();
    }
    let promoted: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_clusters WHERE status='promoted'").fetch_one(&pool).await.unwrap();
    assert_eq!(promoted, 0, "one contributor must never promote a cluster");
}

#[tokio::test]
async fn n_distinct_contributors_promote() {
    let (_c, url) = test_db().await; let pool = entheai_brain::db::connect(&url).await.unwrap();
    let cfg = test_config(); let emb = vec![0.4_f32; 1024];  // n_promote = 3
    for c in ["a","b","c"] {
        entheai_brain::curate::ingest_learning(&pool, &cfg, c, "s", &vecish("y", &emb), None).await.unwrap();
    }
    let (status, seq): (String, Option<i64>) = sqlx::query_as(
        "SELECT status::text, promotion_seq FROM learning_clusters LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(status, "promoted");
    assert!(seq.is_some(), "promoted clusters get a monotonic promotion_seq");
}

#[tokio::test]
async fn one_fleet_contributor_promotes_immediately() {
    let (_c, url) = test_db().await; let pool = entheai_brain::db::connect(&url).await.unwrap();
    let mut cfg = test_config(); cfg.fleet_contributor_ids.insert("fleet1".into());
    let emb = vec![0.5_f32; 1024];
    entheai_brain::curate::ingest_learning(&pool, &cfg, "fleet1", "s", &vecish("z", &emb), None).await.unwrap();
    let promoted: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_clusters WHERE status='promoted'").fetch_one(&pool).await.unwrap();
    assert_eq!(promoted, 1, "a configured fleet contributor is trusted to self-promote");
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test --test api promote`
- [ ] **Step 3: Implement promotion** — after the corroborate/create branches in `ingest_learning`, before returning, call `maybe_promote`:
```rust
async fn maybe_promote(pool: &PgPool, cfg: &Config, cluster_id: uuid::Uuid) -> anyhow::Result<()> {
    // Fleet fast-path: any fleet member on the cluster promotes it.
    let has_fleet: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cluster_members m JOIN contributors c ON c.contributor_id=m.contributor_id
                        WHERE m.cluster_id=$1 AND c.is_fleet)")
        .bind(cluster_id).fetch_one(pool).await?;
    let (dc, status): (i32, String) = sqlx::query_as(
        "SELECT distinct_contributors, status::text FROM learning_clusters WHERE id=$1")
        .bind(cluster_id).fetch_one(pool).await?;
    if status == "promoted" { return Ok(()); }
    if has_fleet || (dc as i64) >= cfg.n_promote {
        sqlx::query(
            "UPDATE learning_clusters
             SET status='promoted', promotion_seq=nextval('promotion_seq_gen'),
                 confidence = least(1.0, distinct_contributors::real / $2), updated_at=now()
             WHERE id=$1 AND status<>'promoted'")
            .bind(cluster_id).bind(cfg.n_promote as f32).execute(pool).await?;
        sqlx::query("UPDATE contributors SET promoted_count = promoted_count + 1
                     WHERE contributor_id IN (SELECT contributor_id FROM cluster_members WHERE cluster_id=$1)")
            .bind(cluster_id).execute(pool).await?;
    }
    Ok(())
}
```
Wire `maybe_promote(pool, cfg, cluster_id).await?;` into both branches (capture `cluster_id` in the corroborate branch too).

- [ ] **Step 4: Run — PASS.** `cargo test --test api promote`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat(curate): corroboration + fleet promotion gate with monotonic seq"`

---

### Task 11: Curation — reputation + contradiction/decay

**Files:** Modify `entheai-brain/src/curate.rs`, `tests/api.rs`

- [ ] **Step 1: Integration test** — a learning with `outcome="failed"` matching a promoted cluster marks it `contradicted` and bumps contributors' `contradicted_count`:
```rust
#[tokio::test]
async fn failed_outcome_contradicts_promoted_cluster() {
    let (_c, url) = test_db().await; let pool = entheai_brain::db::connect(&url).await.unwrap();
    let cfg = test_config(); let emb = vec![0.6_f32; 1024];
    for c in ["a","b","c"] {   // promote it first (succeeded)
        entheai_brain::curate::ingest_learning(&pool, &cfg, c, "s", &vecish("p", &emb), Some("succeeded")).await.unwrap();
    }
    entheai_brain::curate::ingest_learning(&pool, &cfg, "d", "s", &vecish("p", &emb), Some("failed")).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status::text FROM learning_clusters LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(status, "contradicted");
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test --test api contradict`
- [ ] **Step 3: Implement** — in `ingest_learning`, when `outcome == Some("failed")` and a cluster matched, after corroboration run `contradict`:
```rust
async fn contradict(pool: &PgPool, cluster_id: uuid::Uuid) -> anyhow::Result<()> {
    let changed = sqlx::query("UPDATE learning_clusters SET status='contradicted', updated_at=now()
                               WHERE id=$1 AND status='promoted'")
        .bind(cluster_id).execute(pool).await?;
    if changed.rows_affected() == 1 {
        sqlx::query("UPDATE contributors SET contradicted_count = contradicted_count + 1, reputation = greatest(0.0, reputation - 0.5)
                     WHERE contributor_id IN (SELECT contributor_id FROM cluster_members WHERE cluster_id=$1)")
            .bind(cluster_id).execute(pool).await?;
    }
    Ok(())
}
```
Call `contradict` in the matched branch when `outcome == Some("failed")` (instead of / in addition to `maybe_promote`). Add a `decay_stale` free fn (`UPDATE learning_clusters SET status='pending', promotion_seq=NULL WHERE status='promoted' AND updated_at < now() - interval '90 days'`) — call it from the background job (Task 15); no test required beyond compile.

- [ ] **Step 4: Run — PASS.** `cargo test --test api contradict`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat(curate): contradiction demotes + reputation penalty; stale decay"`

---

### Task 12: `POST /v1/learnings` handler

**Files:** Create `entheai-brain/src/routes.rs`; Modify `src/lib.rs`, `main.rs`, `tests/api.rs`

- [ ] **Step 1: End-to-end test** — POST two learnings from two contributors, then assert a cluster exists. Use the real router with a **wiremock embed server** returning a fixed vector. Add to `tests/api.rs`:
```rust
#[tokio::test]
async fn post_learnings_ingests() {
    let (_c, url) = test_db().await;
    let embed = MockServer::start().await;  // wiremock; returns vec![0.7;1024]
    Mock::given(method("POST")).and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[{"embedding": vec![0.7_f32;1024]}]})))
        .mount(&embed).await;
    let app = entheai_brain::app(test_state(&url, &embed.uri()).await);
    let server = axum_test::TestServer::new(app).unwrap();
    let body = serde_json::json!({"contributor_id":"c1","session_id":"s1","learnings":[{"text":"cargo test caught it"}]});
    server.post("/v1/learnings").authorization_bearer("test-token").json(&body).await.assert_status(axum::http::StatusCode::ACCEPTED);
    // second distinct contributor
    let body2 = serde_json::json!({"contributor_id":"c2","session_id":"s2","learnings":[{"text":"cargo test caught it"}]});
    server.post("/v1/learnings").authorization_bearer("test-token").json(&body2).await;
    let pool = entheai_brain::db::connect(&url).await.unwrap();
    let dc: i32 = sqlx::query_scalar("SELECT max(distinct_contributors) FROM learning_clusters").fetch_one(&pool).await.unwrap();
    assert_eq!(dc, 2);
}
```
Add `entheai_brain::app(AppState) -> Router` and `test_state(db_url, embed_url)` helpers.

- [ ] **Step 2: Run — FAIL.** `cargo test --test api post_learnings`
- [ ] **Step 3: Implement `src/routes.rs`** `post_learnings` + `app()`:
```rust
use axum::{extract::State, Json, http::StatusCode, routing::{get, post}, Router, middleware};
use crate::{AppState, error::AppError, wire::*, scrub, curate::{ingest_learning, LearningEmbedded}};

pub async fn post_learnings(State(s): State<AppState>, Json(b): Json<LearningBatch>) -> Result<(StatusCode, Json<Accepted>), AppError> {
    let mut accepted = 0;
    for l in &b.learnings {
        if scrub::contains_hard_secret(&l.text) { continue; }         // silently drop secret-bearing items
        let text = scrub::scrub_text(&l.text);
        let embedding = s.embedder.embed(&text).await.map_err(AppError::Other)?;
        let le = LearningEmbedded { text, tags: l.tags.clone(), embedding };
        ingest_learning(&s.pool, &s.config, &b.contributor_id, &b.session_id, &le, l.outcome.as_deref())
            .await.map_err(AppError::Other)?;
        accepted += 1;
    }
    Ok((StatusCode::ACCEPTED, Json(Accepted { accepted })))
}

pub fn app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/learnings", post(post_learnings))
        // trajectories, sync, stats added in Task 13
        .route_layer(middleware::from_fn_with_state(state.clone(), crate::auth::require_bearer));
    Router::new().route("/health", get(|| async { "ok" })).merge(protected).with_state(state)
}
```
In `main.rs`, build `AppState { pool, config: Arc::new(config), embedder: Arc::new(Embedder::new(&cfg.embed_url)) }` and serve `app(state)`.

- [ ] **Step 4: Run — PASS.** `cargo test --test api post_learnings`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: POST /v1/learnings (scrub → embed → curate) + app router"`

---

### Task 13: `POST /v1/trajectories`, `GET /v1/sync`, `GET /v1/stats`

**Files:** Modify `entheai-brain/src/routes.rs`, `tests/api.rs`

- [ ] **Step 1: Tests** — (a) POST a trajectory → row stored + scrubbed; (b) after promoting a cluster, `GET /v1/sync?since=0` returns it; `since=<that seq>` returns empty. Add to `tests/api.rs`:
```rust
#[tokio::test]
async fn sync_returns_promoted_after_cursor() {
    let (_c, url) = test_db().await;
    let embed = MockServer::start().await;
    Mock::given(method("POST")).and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[{"embedding": vec![0.8_f32;1024]}]}))).mount(&embed).await;
    let app = entheai_brain::app(test_state(&url, &embed.uri()).await);
    let server = axum_test::TestServer::new(app).unwrap();
    for c in ["a","b","c"] {
        let body = serde_json::json!({"contributor_id":c,"session_id":"s","learnings":[{"text":"same"}]});
        server.post("/v1/learnings").authorization_bearer("test-token").json(&body).await;
    }
    let res = server.get("/v1/sync?since=0").authorization_bearer("test-token").await;
    res.assert_status_ok();
    let v: serde_json::Value = res.json();
    assert_eq!(v["learnings"].as_array().unwrap().len(), 1);
    let cursor = v["cursor"].as_i64().unwrap();
    let res2 = server.get(&format!("/v1/sync?since={cursor}")).authorization_bearer("test-token").await;
    assert_eq!(res2.json::<serde_json::Value>()["learnings"].as_array().unwrap().len(), 0);
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test --test api sync_returns`
- [ ] **Step 3: Implement** the three handlers in `src/routes.rs` and add their routes to the protected router:
```rust
#[derive(serde::Deserialize)] pub struct SyncQuery { pub since: i64, #[serde(default = "def_limit")] pub limit: i64 }
fn def_limit() -> i64 { 200 }

pub async fn get_sync(State(s): State<AppState>, axum::extract::Query(q): axum::extract::Query<SyncQuery>) -> Result<Json<SyncResponse>, AppError> {
    let rows: Vec<(uuid::Uuid, String, Vec<String>, f32, i32, i64)> = sqlx::query_as(
        "SELECT id, canonical_text, tags, confidence, distinct_contributors, promotion_seq
         FROM learning_clusters WHERE status='promoted' AND promotion_seq > $1
         ORDER BY promotion_seq ASC LIMIT $2")
        .bind(q.since).bind(q.limit.min(1000)).fetch_all(&s.pool).await?;
    let cursor = rows.last().map(|r| r.5).unwrap_or(q.since);
    let learnings = rows.into_iter().map(|(id, text, tags, confidence, corr, _)| SyncItem {
        id: id.to_string(), text, tags, confidence, corroborations: corr }).collect();
    Ok(Json(SyncResponse { cursor, learnings }))
}

pub async fn post_trajectories(State(s): State<AppState>, Json(b): Json<TrajectoryBatch>) -> Result<(StatusCode, Json<Accepted>), AppError> {
    let mut accepted = 0;
    for mut t in b.trajectories {
        scrub::scrub_json(&mut t);   // from Task 7
        sqlx::query("INSERT INTO trajectories (contributor_id, session_id, payload) VALUES ($1,$2,$3)")
            .bind(&b.contributor_id).bind(&b.session_id).bind(&t).execute(&s.pool).await?;
        accepted += 1;
    }
    Ok((StatusCode::ACCEPTED, Json(Accepted { accepted })))
}

pub async fn get_stats(State(s): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    let (raw, promoted, contribs, traj): (i64,i64,i64,i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM learning_clusters),
                (SELECT count(*) FROM learning_clusters WHERE status='promoted'),
                (SELECT count(*) FROM contributors),
                (SELECT count(*) FROM trajectories)").fetch_one(&s.pool).await?;
    Ok(Json(serde_json::json!({"clusters":raw,"promoted":promoted,"contributors":contribs,"trajectories":traj})))
}
```
Add routes: `.route("/v1/trajectories", post(post_trajectories)).route("/v1/sync", get(get_sync)).route("/v1/stats", get(get_stats))`.

- [ ] **Step 4: Run — PASS.** `cargo test --test api`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: trajectories ingest, /v1/sync cursor, /v1/stats"`

---

### Task 14: Rate limiting

**Files:** Create `entheai-brain/src/ratelimit.rs`; Modify `src/routes.rs`, `tests/api.rs`

- [ ] **Step 1: Test** — the same client exceeding the per-window quota gets `429`. Use a low test quota (set via a `RateLimit::new(2)` per second):
```rust
#[tokio::test]
async fn rate_limit_returns_429() {
    let rl = entheai_brain::ratelimit::RateLimit::new(2);
    let key = "1.2.3.4";
    assert!(rl.check(key).is_ok());
    assert!(rl.check(key).is_ok());
    assert!(rl.check(key).is_err());   // 3rd in the same second
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test --test api rate_limit` (or a unit test in `ratelimit.rs`)
- [ ] **Step 3: Implement `src/ratelimit.rs`** using `governor` keyed by client key (IP or contributor):
```rust
use governor::{Quota, RateLimiter, state::keyed::DefaultKeyedStateStore, clock::DefaultClock};
use std::num::NonZeroU32;
type Keyed = RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>;

pub struct RateLimit(Keyed);
impl RateLimit {
    pub fn new(per_sec: u32) -> Self {
        Self(RateLimiter::keyed(Quota::per_second(NonZeroU32::new(per_sec.max(1)).unwrap())))
    }
    pub fn check(&self, key: &str) -> Result<(), ()> { self.0.check_key(&key.to_string()).map_err(|_| ()) }
}
```
Add a middleware `rate_limit_mw` that extracts the client IP (from `ConnectInfo` or `x-forwarded-for`), calls `state.rate_limit.check(ip)`, returns `AppError::RateLimited` on failure; add `rate_limit: Arc<RateLimit>` to `AppState` and `.route_layer(middleware::from_fn_with_state(...))` on the protected router. Configure the real quota (e.g. 60/s) in `main`.

- [ ] **Step 4: Run — PASS.** `cargo test rate_limit`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: per-client rate limiting (governor)"`

---

### Task 15: HF mirror background job

**Files:** Create `entheai-brain/src/hf_mirror.rs`; Modify `main.rs`, `tests/api.rs`

- [ ] **Step 1: Test** — `export_batch` reads unexported trajectories, POSTs them to a (wiremock) HF endpoint, and marks them exported. Assert the mock received the rows and `exported_to_hf` flips:
```rust
#[tokio::test]
async fn export_batch_marks_exported() {
    let (_c, url) = test_db().await; let pool = entheai_brain::db::connect(&url).await.unwrap();
    sqlx::query("INSERT INTO trajectories (contributor_id, session_id, payload) VALUES ('c','s','{\"a\":1}')").execute(&pool).await.unwrap();
    let hf = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"success":true}))).mount(&hf).await;
    let n = entheai_brain::hf_mirror::export_batch(&pool, &hf.uri(), "tok", "repo", 100).await.unwrap();
    assert_eq!(n, 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM trajectories WHERE exported_to_hf=FALSE").fetch_one(&pool).await.unwrap();
    assert_eq!(left, 0);
}
```

- [ ] **Step 2: Run — FAIL.** `cargo test --test api export_batch`
- [ ] **Step 3: Implement `src/hf_mirror.rs`** (`export_batch` returns count; a `spawn_periodic` loop calls it every N minutes + `curate::decay_stale`). Keep the HF call abstracted behind the passed `base_url` so the test can point at wiremock; real deploy uses the HF datasets `create_commit` upload API. Mark rows exported only after a 2xx.
```rust
pub async fn export_batch(pool: &sqlx::PgPool, hf_base: &str, token: &str, repo: &str, batch: i64) -> anyhow::Result<usize> {
    let rows: Vec<(uuid::Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT id, payload FROM trajectories WHERE exported_to_hf=FALSE ORDER BY created_at LIMIT $1").bind(batch).fetch_all(pool).await?;
    if rows.is_empty() { return Ok(0); }
    let jsonl: String = rows.iter().map(|(_, p)| p.to_string()).collect::<Vec<_>>().join("\n");
    let client = reqwest::Client::new();
    client.post(format!("{hf_base}/api/datasets/{repo}/commit/main"))
        .bearer_auth(token).json(&serde_json::json!({"jsonl": jsonl})).send().await?.error_for_status()?;
    let ids: Vec<uuid::Uuid> = rows.iter().map(|(id, _)| *id).collect();
    sqlx::query("UPDATE trajectories SET exported_to_hf=TRUE WHERE id = ANY($1)").bind(&ids).execute(pool).await?;
    Ok(rows.len())
}
```
NOTE: the exact HF request shape is a placeholder for the real HF Hub commit API — adjust the URL/body to the actual `huggingface_hub` upload contract during deploy; the test only pins the "read → POST → mark exported" behavior. In `main`, if `hf_token` + `hf_repo` are set, `tokio::spawn` a loop calling `export_batch` + `decay_stale` every 15 min.

- [ ] **Step 4: Run — PASS.** `cargo test --test api export_batch`
- [ ] **Step 5: Commit** `git add -A && git commit -m "feat: HF trajectory-mirror job + periodic decay"`

---

### Task 16: Deploy artifacts

**Files:** Create `entheai-brain/deploy/entheai-brain.service`, `entheai-brain/deploy/Dockerfile`, `entheai-brain/README.md`

- [ ] **Step 1: `deploy/Dockerfile`** (multi-stage, static-ish):
```dockerfile
FROM rust:1.83 AS build
WORKDIR /app
COPY . .
RUN cargo build --release
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/entheai-brain /usr/local/bin/entheai-brain
COPY --from=build /app/migrations /migrations
ENV RUST_LOG=info
EXPOSE 8080
CMD ["entheai-brain"]
```
- [ ] **Step 2: `deploy/entheai-brain.service`** (systemd, for a non-Docker deploy on dev-cx53):
```ini
[Unit]
Description=entheai second-brain API
After=network.target postgresql.service
[Service]
EnvironmentFile=/etc/entheai-brain/env
ExecStart=/usr/local/bin/entheai-brain
Restart=on-failure
DynamicUser=yes
[Install]
WantedBy=multi-user.target
```
- [ ] **Step 3: `README.md`** — document env vars (from `.env.example`), `docker build`/`run`, the Postgres+pgvector requirement (`pgvector/pgvector:pg16` image or `CREATE EXTENSION vector`), the endpoints, and that the bearer token is public/embedded (rotate via the env var). No test; verify `cargo build --release` succeeds and `docker build -f deploy/Dockerfile .` builds.
- [ ] **Step 4: Verify** `cargo build --release` succeeds.
- [ ] **Step 5: Commit** `git add -A && git commit -m "chore: deploy artifacts (Dockerfile, systemd, README)"`

---

## Self-Review

**Spec coverage:** §1 role → Tasks 1,12,13. §2 two streams → Tasks 12 (learnings), 13 (trajectories). §3 trust/curation → Tasks 9,10,11 (cluster, promote, reputation/contradiction), anonymous `contributor_id` throughout. §4 sync → Task 13. §5 stack/tables → Tasks 1,3. §6 API → Tasks 12,13 (+health Task 1). §7 auth/PII/abuse → Tasks 6 (auth), 7 (scrub), 14 (rate limit). §8 HF mirror → Task 15. §9 deploy/separate-repo → Tasks 1,16. §10 testing → every task (unit + testcontainers integration + wiremock). §12 success criteria → covered by the Task 12/13 e2e + Task 10 invariants. **No gaps.**

**Placeholder scan:** the only intentional "adjust at deploy" note is the HF Hub request shape (Task 15) — flagged explicitly because the exact `huggingface_hub` commit contract must be confirmed against the live API; the *tested behavior* (read→POST→mark) is concrete. Embedding dim (1024) is called out as model-dependent. No `TODO`/"handle edge cases" placeholders.

**Type consistency:** `LearningEmbedded { text, tags, embedding }`, `ingest_learning(pool, cfg, contributor_id, session_id, &LearningEmbedded, outcome)`, `AppState { pool, config, embedder, rate_limit }`, `Config { n_promote: i64, tau_match: f32, fleet_contributor_ids: HashSet<String>, … }`, `promotion_seq` cursor, `SyncItem`/`SyncResponse` are used consistently across Tasks 9–15.
