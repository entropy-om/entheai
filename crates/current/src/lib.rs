//! entheai-current — current-awareness ingestion. AHOGY A DOLGOK VANNAK: the
//! brain should know the world as it IS, so two live sources feed the raw
//! memory soil under hard, honest daily budgets:
//!
//!   * **Valyu** (`POST /v1/search`, `x-api-key`) — AI-native search across
//!     web/news/proprietary indexes, priced per result (`max_price` caps CPM).
//!   * **WorldMonitor** (`api.worldmonitor.app`, `X-WorldMonitor-Key`) — the
//!     news feed digest, ACLED conflict events, and natural-disaster events.
//!
//! Every request is metered through a persistent [`BudgetLedger`] that resets
//! at local midnight and HARD-STOPS at the configured caps (WorldMonitor's cap
//! clamps to ≤ 50/day — the operator's mandate). When a budget is spent the
//! engine says so and does nothing — it never borrows against tomorrow.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One fetched item of current-world knowledge, normalized across sources.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentItem {
    /// "valyu" | "worldmonitor"
    pub source: String,
    /// "news" | "conflict" | "natural" | "search"
    pub kind: String,
    pub title: String,
    pub url: Option<String>,
    /// Markdown/plain body — what gets ingested into the raw store.
    pub content: String,
    /// Publication date when the source provides one (ISO-8601-ish, verbatim).
    pub published_at: Option<String>,
}

/// What one pulse actually did — counts, not claims.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PulseReport {
    pub requests_spent: Vec<(String, u32)>,
    pub items: Vec<CurrentItem>,
    /// Sources skipped because their daily budget is exhausted.
    pub budget_exhausted: Vec<String>,
    /// Sources skipped because no API key resolved.
    pub keyless: Vec<String>,
    /// Per-source fetch errors (source, error) — surfaced, never swallowed.
    pub errors: Vec<(String, String)>,
}

impl PulseReport {
    /// One honest status line for the TUI.
    pub fn summary(&self) -> String {
        let spent: u32 = self.requests_spent.iter().map(|(_, n)| n).sum();
        let mut s = format!(
            "current: {} item(s) from {} request(s)",
            self.items.len(),
            spent
        );
        if !self.budget_exhausted.is_empty() {
            s.push_str(&format!(
                " · budget exhausted: {}",
                self.budget_exhausted.join(", ")
            ));
        }
        if !self.keyless.is_empty() {
            s.push_str(&format!(" · no key: {}", self.keyless.join(", ")));
        }
        if !self.errors.is_empty() {
            s.push_str(&format!(" · errors: {}", self.errors.len()));
        }
        s
    }
}

/// Today's UTC civil date as "YYYY-MM-DD" — shared by the budget ledger and
/// anything naming context folders (karmapa-chenno).
pub fn utc_today() -> String {
    utc_date_days_ago(0)
}

/// UTC civil date `n` days ago as "YYYY-MM-DD" — no chrono dep. UTC is a
/// stable, honest boundary for budget resets and event windows.
/// Civil-from-days per Howard Hinnant's algorithm, pure integer math.
fn utc_date_days_ago(n: u64) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    utc_date_from_secs(secs.saturating_sub(n * 86_400))
}

fn utc_date_from_secs(secs: u64) -> String {
    let days = secs / 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// A JSON date value → display string. Strings pass through verbatim; numbers
/// are treated as epoch seconds (or milliseconds when > 1e12) and rendered as
/// a UTC date — WorldMonitor mixes both.
fn date_from_json(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    let n = v.as_f64()?;
    if n <= 0.0 {
        return None;
    }
    let secs = if n > 1.0e12 { n / 1000.0 } else { n };
    Some(utc_date_from_secs(secs as u64))
}

// ---------------------------------------------------------------------------
// Budget ledger
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
struct LedgerState {
    /// Local date "YYYY-MM-DD" the counts belong to; a new day resets them.
    date: String,
    counts: std::collections::HashMap<String, u32>,
}

/// Persistent per-source daily request meter. Load-modify-save on every spend
/// (pulses are minutes apart — durability beats write elegance here).
pub struct BudgetLedger {
    path: PathBuf,
    caps: std::collections::HashMap<String, u32>,
}

impl BudgetLedger {
    pub fn new(path: PathBuf, caps: impl IntoIterator<Item = (String, u32)>) -> Self {
        Self {
            path,
            caps: caps.into_iter().collect(),
        }
    }

    fn today() -> String {
        utc_date_days_ago(0)
    }

    fn load(&self) -> LedgerState {
        let state: LedgerState = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if state.date != Self::today() {
            LedgerState {
                date: Self::today(),
                counts: Default::default(),
            }
        } else {
            state
        }
    }

    fn save(&self, state: &LedgerState) {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(json) = serde_json::to_string_pretty(state) {
            if let Err(e) = std::fs::write(&self.path, json) {
                log::warn!("current: budget ledger write failed (continuing): {e}");
            }
        }
    }

    /// Spend `n` requests from `source`'s budget. Returns false — and spends
    /// NOTHING — when the remaining budget can't cover all `n`.
    pub fn try_spend(&self, source: &str, n: u32) -> bool {
        let cap = match self.caps.get(source) {
            Some(c) => *c,
            None => return false, // unknown source: nothing budgeted, nothing spent
        };
        let mut state = self.load();
        let used = state.counts.get(source).copied().unwrap_or(0);
        if used + n > cap {
            return false;
        }
        state.counts.insert(source.to_string(), used + n);
        self.save(&state);
        true
    }

    /// (used, cap) for a source today.
    pub fn status(&self, source: &str) -> (u32, u32) {
        let used = self.load().counts.get(source).copied().unwrap_or(0);
        (used, self.caps.get(source).copied().unwrap_or(0))
    }
}

// ---------------------------------------------------------------------------
// Valyu client
// ---------------------------------------------------------------------------

pub struct ValyuClient {
    base: String,
    api_key: String,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct ValyuResponse {
    success: bool,
    error: Option<String>,
    #[serde(default)]
    results: Vec<ValyuResult>,
}

#[derive(Debug, Deserialize)]
struct ValyuResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
    publication_date: Option<String>,
}

impl ValyuClient {
    pub fn new(base: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }

    /// One `news`-scoped search. `max_price` is the CPM ceiling Valyu may
    /// charge for this query — cost honesty is part of the request itself.
    pub async fn search_news(
        &self,
        query: &str,
        max_results: u32,
        max_price: f64,
    ) -> anyhow::Result<Vec<CurrentItem>> {
        let body = serde_json::json!({
            "query": query,
            "search_type": "news",
            "max_num_results": max_results,
            "response_length": "medium",
            "relevance_threshold": 0.5,
            "max_price": max_price,
        });
        let resp: ValyuResponse = self
            .http
            .post(format!("{}/v1/search", self.base))
            .header("x-api-key", &self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if !resp.success {
            anyhow::bail!(
                "valyu: {}",
                resp.error.unwrap_or_else(|| "unknown error".into())
            );
        }
        Ok(resp
            .results
            .into_iter()
            .filter_map(|r| {
                let content = r.content?;
                Some(CurrentItem {
                    source: "valyu".into(),
                    kind: "search".into(),
                    title: r.title.unwrap_or_else(|| "(untitled)".into()),
                    url: r.url,
                    content,
                    published_at: r.publication_date,
                })
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// WorldMonitor client
// ---------------------------------------------------------------------------

pub struct WorldMonitorClient {
    base: String,
    api_key: String,
    http: reqwest::Client,
}

impl WorldMonitorClient {
    pub fn new(base: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }

    async fn get_json(&self, path_and_query: &str) -> anyhow::Result<serde_json::Value> {
        Ok(self
            .http
            .get(format!("{}{path_and_query}", self.base))
            .header("X-WorldMonitor-Key", &self.api_key)
            .header(
                "User-Agent",
                "entheai/1.x (+https://github.com/entropy-om/entheai)",
            )
            .header("Accept", "application/json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Normalize a JSON array of objects into items, best-effort per entry:
    /// entries missing a recognizable title are skipped, not invented.
    fn items_from(v: serde_json::Value, kind: &str, cap: usize) -> Vec<CurrentItem> {
        let arr = v
            .as_array()
            .cloned()
            .or_else(|| {
                v.as_object().and_then(|o| {
                    ["items", "events", "articles", "data", "results"]
                        .iter()
                        .find_map(|k| o.get(*k).and_then(|x| x.as_array()).cloned())
                })
            })
            .unwrap_or_default();
        arr.into_iter()
            .take(cap)
            .filter_map(|e| {
                let title = ["title", "headline", "event_type", "name", "summary"]
                    .iter()
                    .find_map(|k| e.get(*k).and_then(|x| x.as_str()))?
                    .to_string();
                let url = ["url", "link", "source_url"]
                    .iter()
                    .find_map(|k| e.get(*k).and_then(|x| x.as_str()))
                    .map(|s| s.to_string());
                let published_at = ["published_at", "publishedAt", "date", "event_date", "time"]
                    .iter()
                    .find_map(|k| e.get(*k).and_then(date_from_json));
                // The raw entry IS the content — never rewritten, per the soil rule.
                let content = serde_json::to_string_pretty(&e).ok()?;
                Some(CurrentItem {
                    source: "worldmonitor".into(),
                    kind: kind.into(),
                    title,
                    url,
                    content,
                    published_at,
                })
            })
            .collect()
    }

    /// The feed digest nests articles under `categories.<name>.items[]` with
    /// `importanceScore` per article — rank globally by importance, take `cap`.
    pub async fn news_digest(&self, cap: usize) -> anyhow::Result<Vec<CurrentItem>> {
        let v = self
            .get_json("/api/news/v1/list-feed-digest?variant=full&lang=en")
            .await?;
        let mut scored: Vec<(f64, CurrentItem)> = Vec::new();
        if let Some(categories) = v.get("categories").and_then(|c| c.as_object()) {
            for (cat, entry) in categories {
                let Some(items) = entry.get("items").and_then(|i| i.as_array()) else {
                    continue;
                };
                for e in items {
                    let Some(title) = e.get("title").and_then(|t| t.as_str()) else {
                        continue;
                    };
                    let score = e
                        .get("importanceScore")
                        .and_then(|s| s.as_f64())
                        .unwrap_or(0.0);
                    let mut item_json = e.clone();
                    if let Some(o) = item_json.as_object_mut() {
                        o.insert("category".into(), serde_json::json!(cat));
                    }
                    scored.push((
                        score,
                        CurrentItem {
                            source: "worldmonitor".into(),
                            kind: "news".into(),
                            title: title.to_string(),
                            url: e.get("link").and_then(|l| l.as_str()).map(String::from),
                            content: serde_json::to_string_pretty(&item_json).unwrap_or_default(),
                            published_at: e.get("publishedAt").and_then(date_from_json),
                        },
                    ));
                }
            }
        }
        scored.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored.into_iter().take(cap).map(|(_, i)| i).collect())
    }

    pub async fn conflict_events(&self, cap: usize) -> anyhow::Result<Vec<CurrentItem>> {
        // ACLED needs an explicit window — the last 3 days keeps it CURRENT.
        let v = self
            .get_json(&format!(
                "/api/conflict/v1/list-acled-events?page_size=25&start={}&end={}",
                utc_date_days_ago(3),
                utc_date_days_ago(0),
            ))
            .await?;
        Ok(Self::items_from(v, "conflict", cap))
    }

    pub async fn natural_events(&self, cap: usize) -> anyhow::Result<Vec<CurrentItem>> {
        let v = self
            .get_json("/api/natural/v1/list-natural-events?days=2")
            .await?;
        Ok(Self::items_from(v, "natural", cap))
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Per-pulse item cap per WorldMonitor endpoint (keeps a pulse's ingest bounded).
const WM_ITEMS_PER_ENDPOINT: usize = 12;

pub struct CurrentEngine {
    valyu: Option<ValyuClient>,
    worldmonitor: Option<WorldMonitorClient>,
    ledger: BudgetLedger,
    topics: Vec<String>,
    valyu_max_results: u32,
    valyu_max_price: f64,
}

impl CurrentEngine {
    /// Build from config: keys resolve from the configured env vars; a missing
    /// key disables that source (reported per-pulse in `keyless`, not hidden).
    pub fn from_config(cfg: &entheai_config::CurrentConfig, ledger_path: PathBuf) -> Self {
        let valyu = std::env::var(&cfg.valyu_api_key_env)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .map(|k| ValyuClient::new("https://api.valyu.ai", k));
        let worldmonitor = std::env::var(&cfg.worldmonitor_api_key_env)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .map(|k| WorldMonitorClient::new("https://api.worldmonitor.app", k));
        // The operator's mandate: WorldMonitor never exceeds 50/day, whatever
        // the config says.
        let wm_cap = cfg.worldmonitor_daily_cap.min(50);
        let ledger = BudgetLedger::new(
            ledger_path,
            [
                ("valyu".to_string(), cfg.valyu_daily_cap),
                ("worldmonitor".to_string(), wm_cap),
            ],
        );
        Self {
            valyu,
            worldmonitor,
            ledger,
            topics: cfg.topics.clone(),
            valyu_max_results: cfg.valyu_max_results,
            valyu_max_price: cfg.valyu_max_price,
        }
    }

    #[doc(hidden)]
    pub fn with_clients(
        valyu: Option<ValyuClient>,
        worldmonitor: Option<WorldMonitorClient>,
        ledger: BudgetLedger,
        topics: Vec<String>,
    ) -> Self {
        Self {
            valyu,
            worldmonitor,
            ledger,
            topics,
            valyu_max_results: 5,
            valyu_max_price: 30.0,
        }
    }

    pub fn budget_status(&self) -> Vec<(String, u32, u32)> {
        ["valyu", "worldmonitor"]
            .iter()
            .map(|s| {
                let (used, cap) = self.ledger.status(s);
                (s.to_string(), used, cap)
            })
            .collect()
    }

    /// One pulse: fetch what the budgets allow, return everything that
    /// happened. The caller ingests `report.items` into the soil.
    pub async fn pulse(&self) -> PulseReport {
        let mut report = PulseReport::default();

        // WorldMonitor: three endpoints = three requests, spent atomically —
        // a partial pulse would skew the feed toward whichever endpoint is
        // cheapest to reach.
        match &self.worldmonitor {
            None => report.keyless.push("worldmonitor".into()),
            Some(wm) => {
                if self.ledger.try_spend("worldmonitor", 3) {
                    report.requests_spent.push(("worldmonitor".into(), 3));
                    for (name, res) in [
                        ("news", wm.news_digest(WM_ITEMS_PER_ENDPOINT).await),
                        ("conflict", wm.conflict_events(WM_ITEMS_PER_ENDPOINT).await),
                        ("natural", wm.natural_events(WM_ITEMS_PER_ENDPOINT).await),
                    ] {
                        match res {
                            Ok(items) => report.items.extend(items),
                            Err(e) => report
                                .errors
                                .push((format!("worldmonitor/{name}"), e.to_string())),
                        }
                    }
                } else {
                    report.budget_exhausted.push("worldmonitor".into());
                }
            }
        }

        // Valyu: one request per configured topic.
        match &self.valyu {
            None => {
                if !self.topics.is_empty() {
                    report.keyless.push("valyu".into());
                }
            }
            Some(valyu) => {
                for topic in &self.topics {
                    if !self.ledger.try_spend("valyu", 1) {
                        report.budget_exhausted.push("valyu".into());
                        break;
                    }
                    report.requests_spent.push(("valyu".into(), 1));
                    match valyu
                        .search_news(topic, self.valyu_max_results, self.valyu_max_price)
                        .await
                    {
                        Ok(items) => report.items.extend(items),
                        Err(e) => report.errors.push(("valyu".into(), e.to_string())),
                    }
                }
            }
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_enforces_caps_resets_daily_and_never_partially_spends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget.json");
        let ledger = BudgetLedger::new(path.clone(), [("worldmonitor".to_string(), 5u32)]);

        assert!(ledger.try_spend("worldmonitor", 3));
        assert_eq!(ledger.status("worldmonitor"), (3, 5));
        // 3 more would exceed 5 → refused, and NOTHING spent.
        assert!(!ledger.try_spend("worldmonitor", 3));
        assert_eq!(ledger.status("worldmonitor"), (3, 5));
        assert!(ledger.try_spend("worldmonitor", 2));
        assert!(!ledger.try_spend("worldmonitor", 1), "cap is a hard stop");
        // Unknown sources have no budget at all.
        assert!(!ledger.try_spend("mystery", 1));

        // A ledger from "yesterday" resets today.
        std::fs::write(
            &path,
            r#"{"date":"2001-01-01","counts":{"worldmonitor":5}}"#,
        )
        .unwrap();
        assert!(ledger.try_spend("worldmonitor", 5), "new day, fresh budget");
    }

    #[test]
    fn today_is_a_plausible_utc_date() {
        let d = BudgetLedger::today();
        // 2026-xx-xx shape, correct field widths.
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"), "unexpected date {d}");
        assert_eq!(&d[4..5], "-");
        assert_eq!(&d[7..8], "-");
    }

    #[test]
    fn worldmonitor_items_normalize_arrays_and_wrapped_objects() {
        let wrapped = serde_json::json!({
            "items": [
                {"title": "Quake M6.1", "url": "https://x/1", "date": "2026-07-23"},
                {"headline": "Flood alert", "link": "https://x/2"},
                {"no_title_here": true}
            ]
        });
        let items = WorldMonitorClient::items_from(wrapped, "natural", 10);
        assert_eq!(
            items.len(),
            2,
            "entry without any title-ish field is skipped"
        );
        assert_eq!(items[0].title, "Quake M6.1");
        assert_eq!(items[0].url.as_deref(), Some("https://x/1"));
        assert_eq!(items[0].published_at.as_deref(), Some("2026-07-23"));
        assert_eq!(items[1].title, "Flood alert");
        assert!(
            items[0].content.contains("Quake"),
            "raw entry preserved as content"
        );

        let bare = serde_json::json!([{"title": "t"}]);
        assert_eq!(WorldMonitorClient::items_from(bare, "news", 10).len(), 1);
        // The per-pulse cap holds.
        let many = serde_json::json!((0..30)
            .map(|i| serde_json::json!({"title": format!("t{i}")}))
            .collect::<Vec<_>>());
        assert_eq!(WorldMonitorClient::items_from(many, "news", 12).len(), 12);
    }

    #[tokio::test]
    async fn pulse_reports_keyless_sources_and_spends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = BudgetLedger::new(
            dir.path().join("b.json"),
            [("valyu".to_string(), 5), ("worldmonitor".to_string(), 5)],
        );
        let engine = CurrentEngine::with_clients(None, None, ledger, vec!["rust".into()]);
        let report = engine.pulse().await;
        assert!(report.items.is_empty());
        assert_eq!(report.keyless, vec!["worldmonitor", "valyu"]);
        assert!(report.requests_spent.is_empty(), "keyless spends nothing");
        assert_eq!(engine.budget_status()[0], ("valyu".into(), 0, 5));
    }

    #[tokio::test]
    async fn pulse_fetches_ingests_and_meters_against_wiremock() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .and(header("x-api-key", "vk"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true, "error": null,
                "results": [{"title": "Rust 2.0 shipped", "url": "https://r/1",
                             "content": "body md", "publication_date": "2026-07-23"}]
            })))
            .mount(&server)
            .await;
        // The digest uses its REAL nested shape (categories.<cat>.items[]);
        // conflict/natural use the events-wrapper shape the API serves.
        Mock::given(method("GET"))
            .and(path("/api/news/v1/list-feed-digest"))
            .and(header("X-WorldMonitor-Key", "wmk"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "generatedAt": 1784800000000i64,
                "categories": {
                    "politics": {"items": [
                        {"title": "Summit convened", "link": "https://n/1",
                         "publishedAt": 1784787480000i64, "importanceScore": 72}
                    ]},
                    "tech": {"items": []}
                }
            })))
            .mount(&server)
            .await;
        for p in [
            "/api/conflict/v1/list-acled-events",
            "/api/natural/v1/list-natural-events",
        ] {
            Mock::given(method("GET"))
                .and(path(p))
                .and(header("X-WorldMonitor-Key", "wmk"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "events": [{"title": format!("event from {p}"), "date": 1784787480000i64}]
                })))
                .mount(&server)
                .await;
        }

        let dir = tempfile::tempdir().unwrap();
        let ledger = BudgetLedger::new(
            dir.path().join("b.json"),
            [("valyu".to_string(), 10), ("worldmonitor".to_string(), 50)],
        );
        let engine = CurrentEngine::with_clients(
            Some(ValyuClient::new(server.uri(), "vk")),
            Some(WorldMonitorClient::new(server.uri(), "wmk")),
            ledger,
            vec!["rust releases".into()],
        );

        let report = engine.pulse().await;
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert_eq!(report.items.len(), 4, "1 digest + 2 events + 1 Valyu");
        assert!(report.items.iter().any(|i| i.source == "valyu"));
        assert_eq!(
            report
                .items
                .iter()
                .filter(|i| i.source == "worldmonitor")
                .count(),
            3
        );
        // Epoch-ms dates render as UTC civil dates.
        let news = report.items.iter().find(|i| i.kind == "news").unwrap();
        assert_eq!(news.title, "Summit convened");
        assert_eq!(news.published_at.as_deref(), Some("2026-07-23"));
        // Metering: 3 WM + 1 Valyu.
        let status = engine.budget_status();
        assert_eq!(status[0], ("valyu".into(), 1, 10));
        assert_eq!(status[1], ("worldmonitor".into(), 3, 50));
        assert!(report.summary().contains("4 item(s)"));
    }
}
