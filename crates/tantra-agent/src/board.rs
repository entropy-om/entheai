//! The tantric board — a GitHub-issues-backed kanban at
//! `peterlodri-sec/mlxquantlovefrom.com` (the tantric board of
//! https://mlxquantlovefrom.com). Cards are issues carrying one of the four
//! lane labels (`backlog` / `burning` / `tantra` / `done`); each collaborator's
//! private todo / daily summary is a label-less issue titled
//! `todo: <login>` / `daily: <login>`.
//!
//! Read access uses the repo's public read (no token) when possible; the repo
//! is currently private, so reads fall back to whichever `TANTRIC_TOKEN_*` is
//! set. Writes (add / move / todo / summary) always require one of the three
//! collaborator tokens:
//!
//! - `TANTRIC_TOKEN_PETER` → `peterlodri-sec`
//! - `TANTRIC_TOKEN_8BIT` → `8bit-wraith`
//! - `TANTRIC_TOKEN_SG` → `standardgalactic`

use anyhow::{anyhow, bail, Context};
use serde::Deserialize;

/// GitHub REST API base.
pub const BASE_URL: &str = "https://api.github.com";
/// The board's GitHub repo (`owner/repo`).
pub const REPO: &str = "peterlodri-sec/mlxquantlovefrom.com";
/// The three collaborators: `(env var carrying the token, GitHub login)`.
/// Only these three accounts may write to the board.
pub const COLLABORATORS: [(&str, &str); 3] = [
    ("TANTRIC_TOKEN_PETER", "peterlodri-sec"),
    ("TANTRIC_TOKEN_8BIT", "8bit-wraith"),
    ("TANTRIC_TOKEN_SG", "standardgalactic"),
];

/// The four board lanes, mapped 1:1 to the pre-existing GitHub labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Backlog,
    Burning,
    Tantra,
    Done,
}

impl Lane {
    /// All lanes, in board display order.
    pub const ALL: [Lane; 4] = [Lane::Backlog, Lane::Burning, Lane::Tantra, Lane::Done];

    /// The GitHub label name for this lane.
    pub fn label(self) -> &'static str {
        match self {
            Lane::Backlog => "backlog",
            Lane::Burning => "burning",
            Lane::Tantra => "tantra",
            Lane::Done => "done",
        }
    }

    /// Parse a lane from user input (label name, case-insensitive).
    pub fn parse(s: &str) -> Option<Lane> {
        let lower = s.to_ascii_lowercase();
        Lane::ALL.iter().copied().find(|lane| lane.label() == lower)
    }
}

impl std::fmt::Display for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// The active collaborator, resolved from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collaborator {
    /// The env var the token came from (e.g. `TANTRIC_TOKEN_PETER`).
    pub env_var: &'static str,
    /// The GitHub login of the collaborator.
    pub login: &'static str,
    /// The raw token (never printed).
    pub token: String,
}

/// Resolve the active collaborator from the environment: the first of the
/// three `TANTRIC_TOKEN_*` vars that is set and non-empty wins.
pub fn collaborator_from_env() -> Option<Collaborator> {
    for (env_var, login) in COLLABORATORS {
        if let Ok(token) = std::env::var(env_var) {
            if !token.trim().is_empty() {
                return Some(Collaborator {
                    env_var,
                    login,
                    token,
                });
            }
        }
    }
    None
}

/// The token needed for write access, with a helpful error when absent.
pub fn require_token() -> anyhow::Result<Collaborator> {
    collaborator_from_env().ok_or_else(|| {
        anyhow!(
            "no tantric token in env — set one of TANTRIC_TOKEN_PETER, \
             TANTRIC_TOKEN_8BIT, TANTRIC_TOKEN_SG (only the three board \
             collaborators have write access)"
        )
    })
}

/// Extract the `rel="next"` URL from a GitHub `Link` response header
/// (`<url1>; rel="prev", <url2>; rel="next", ...`), or `None` on the last
/// page / when the header is absent.
fn next_link(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let raw = headers.get(reqwest::header::LINK)?.to_str().ok()?;
    raw.split(',').find_map(|part| {
        let part = part.trim();
        if !part.ends_with("rel=\"next\"") {
            return None;
        }
        let start = part.find('<')?;
        let end = part.find('>')?;
        (start < end).then(|| part[start + 1..end].to_string())
    })
}

/// Build an absolute GitHub API URL for a repo-relative path
/// (e.g. `/issues`, `/issues/3/comments`).
pub fn api_url(path: &str) -> String {
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        format!("{BASE_URL}/repos/{REPO}")
    } else {
        format!("{BASE_URL}/repos/{REPO}/{path}")
    }
}

/// URL of the repo's issues collection (list + create).
pub fn issues_url() -> String {
    api_url("/issues")
}

/// URL of a single issue (move / label updates).
pub fn issue_url(number: u64) -> String {
    api_url(&format!("/issues/{number}"))
}

/// URL of an issue's comments (todo/daily append).
pub fn comments_url(number: u64) -> String {
    api_url(&format!("/issues/{number}/comments"))
}

/// The per-collaborator todo issue title for a login.
pub fn todo_title(login: &str) -> String {
    format!("todo: {login}")
}

/// The per-collaborator daily-summary issue title for a login.
pub fn daily_title(login: &str) -> String {
    format!("daily: {login}")
}

/// Find the first open issue whose title starts with `prefix`.
pub fn find_by_title_prefix<'a>(issues: &'a [GhIssue], prefix: &str) -> Option<&'a GhIssue> {
    issues.iter().find(|issue| issue.title.starts_with(prefix))
}

/// A GitHub label as returned by the issues API.
#[derive(Debug, Clone, Deserialize)]
pub struct GhLabel {
    pub name: String,
}

/// An issue (card, todo, or daily summary).
#[derive(Debug, Clone, Deserialize)]
pub struct GhIssue {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub labels: Vec<GhLabel>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    /// Present on pull requests; `None` on issues.
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

impl GhIssue {
    /// The lane label of a card, or `None` for label-less scratch issues.
    pub fn lane_label(&self) -> Option<&str> {
        self.labels
            .iter()
            .map(|label| label.name.as_str())
            .find(|name| Lane::parse(name).is_some())
    }
}

/// An issue comment (used to append todo items / daily summary entries).
#[derive(Debug, Clone, Deserialize)]
pub struct GhComment {
    pub id: u64,
    pub body: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// The GitHub-issues-backed board client.
#[derive(Debug, Clone)]
pub struct Board {
    client: reqwest::Client,
    token: Option<String>,
}

impl Board {
    /// Build a board client. `token` is optional: reads work without one
    /// (public read), writes require it.
    pub fn new(token: Option<String>) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("tantra-agent")
            // No timeout previously: a stalled GitHub call hung the CLI/agent
            // indefinitely.
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .context("building reqwest client")?;
        Ok(Self { client, token })
    }

    /// Board from the environment: uses the active tantric token if any.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::new(collaborator_from_env().map(|c| c.token))
    }

    /// Board from the environment, requiring a collaborator token (writes).
    pub fn require_token() -> anyhow::Result<Self> {
        Self::new(Some(require_token()?.token))
    }

    /// Whether this board has a token for authenticated requests.
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// The repo this board is backed by.
    pub fn repo(&self) -> &'static str {
        REPO
    }

    /// List all open issues on the board (cards + todo/daily scratch issues).
    /// Paginated — past 100 open issues, a single page used to miss the
    /// `todo:`/`daily:` scratch issue and silently drop cards.
    pub async fn list_issues(&self) -> anyhow::Result<Vec<GhIssue>> {
        let url = format!("{}?state=open&per_page=100", issues_url());
        self.get_all_pages(&url).await
    }

    /// Create an issue: a card when `labels` is non-empty, or a label-less
    /// todo/daily scratch issue.
    pub async fn create_issue(
        &self,
        title: &str,
        body: &str,
        labels: &[&str],
    ) -> anyhow::Result<GhIssue> {
        let mut payload = serde_json::json!({ "title": title });
        if !body.is_empty() {
            payload["body"] = serde_json::Value::String(body.to_string());
        }
        if !labels.is_empty() {
            payload["labels"] = serde_json::Value::Array(
                labels
                    .iter()
                    .map(|label| serde_json::Value::String(label.to_string()))
                    .collect(),
            );
        }
        let resp = self
            .ensure_ok(self.post(&issues_url(), &payload).await?)
            .await?;
        resp.json().await.context("parsing created issue")
    }

    /// Replace an issue's labels — the `move` operation.
    pub async fn update_labels(&self, number: u64, labels: &[&str]) -> anyhow::Result<GhIssue> {
        let payload = serde_json::json!({ "labels": labels });
        let resp = self
            .ensure_ok(self.patch(&issue_url(number), &payload).await?)
            .await?;
        resp.json().await.context("parsing updated issue")
    }

    /// Append a comment to an issue (the todo / daily "append").
    pub async fn add_comment(&self, number: u64, body: &str) -> anyhow::Result<GhComment> {
        let payload = serde_json::json!({ "body": body });
        let resp = self
            .ensure_ok(self.post(&comments_url(number), &payload).await?)
            .await?;
        resp.json().await.context("parsing created comment")
    }

    /// All comments on an issue, newest last. Paginated — with no `per_page`
    /// GitHub's default of 30 applied, so `todo list` silently showed only
    /// the first 30 (the doc claim was "all comments").
    pub async fn list_comments(&self, number: u64) -> anyhow::Result<Vec<GhComment>> {
        let url = format!("{}?per_page=100", comments_url(number));
        self.get_all_pages(&url).await
    }

    /// Follow GitHub's `Link: <url>; rel="next"` pagination header across
    /// pages, collecting and concatenating each page's JSON array. Every
    /// caller passes a `per_page=100`-qualified first-page URL; this handles
    /// whatever's past that first page.
    async fn get_all_pages<T: serde::de::DeserializeOwned>(
        &self,
        first_url: &str,
    ) -> anyhow::Result<Vec<T>> {
        let mut items = Vec::new();
        let mut next_url = Some(first_url.to_string());
        while let Some(url) = next_url {
            let resp = self.ensure_ok(self.get(&url).await?).await?;
            next_url = next_link(resp.headers());
            let page: Vec<T> = resp.json().await.context("parsing paginated page")?;
            items.extend(page);
        }
        Ok(items)
    }

    // --- internal HTTP helpers -------------------------------------------------

    async fn get(&self, url: &str) -> anyhow::Result<reqwest::Response> {
        let mut req = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        req.send().await.with_context(|| format!("GET {url}"))
    }

    async fn post(
        &self,
        url: &str,
        payload: &serde_json::Value,
    ) -> anyhow::Result<reqwest::Response> {
        let mut req = self
            .client
            .post(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(payload);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        req.send().await.with_context(|| format!("POST {url}"))
    }

    async fn patch(
        &self,
        url: &str,
        payload: &serde_json::Value,
    ) -> anyhow::Result<reqwest::Response> {
        let mut req = self
            .client
            .patch(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(payload);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        req.send().await.with_context(|| format!("PATCH {url}"))
    }

    /// Fail on a non-success status, reading the API error body. When no token
    /// is set, a 401/403/404 (private-repo hides) gets a hint about the
    /// collaborator tokens.
    async fn ensure_ok(&self, resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let text = resp.text().await.unwrap_or_default();
        let status_code = status.as_u16();
        if self.token.is_none() && matches!(status_code, 401 | 403 | 404) {
            bail!(
                "GitHub API {status_code}: {text} — the board repo is private; set one of \
                 TANTRIC_TOKEN_PETER / TANTRIC_TOKEN_8BIT / TANTRIC_TOKEN_SG to read it"
            );
        }
        bail!("GitHub API {status_code}: {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_builds_repo_absolute_url() {
        assert_eq!(
            api_url("/issues"),
            "https://api.github.com/repos/peterlodri-sec/mlxquantlovefrom.com/issues"
        );
        assert_eq!(
            api_url("/issues/3/comments"),
            "https://api.github.com/repos/peterlodri-sec/mlxquantlovefrom.com/issues/3/comments"
        );
        assert_eq!(
            issues_url(),
            "https://api.github.com/repos/peterlodri-sec/mlxquantlovefrom.com/issues"
        );
        assert_eq!(
            issue_url(3),
            "https://api.github.com/repos/peterlodri-sec/mlxquantlovefrom.com/issues/3"
        );
        assert_eq!(
            comments_url(3),
            "https://api.github.com/repos/peterlodri-sec/mlxquantlovefrom.com/issues/3/comments"
        );
    }

    fn header_map(link: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::LINK, link.parse().unwrap());
        h
    }

    #[test]
    fn next_link_extracts_the_next_url_from_a_multi_rel_header() {
        let h = header_map(
            r#"<https://api.github.com/repos/x/y/issues?page=1>; rel="prev", <https://api.github.com/repos/x/y/issues?page=3>; rel="next", <https://api.github.com/repos/x/y/issues?page=5>; rel="last""#,
        );
        assert_eq!(
            next_link(&h).as_deref(),
            Some("https://api.github.com/repos/x/y/issues?page=3")
        );
    }

    #[test]
    fn next_link_is_none_on_the_last_page() {
        let h = header_map(
            r#"<https://api.github.com/repos/x/y/issues?page=1>; rel="prev", <https://api.github.com/repos/x/y/issues?page=2>; rel="first""#,
        );
        assert!(next_link(&h).is_none());
    }

    #[test]
    fn next_link_is_none_when_the_header_is_absent() {
        assert!(next_link(&reqwest::header::HeaderMap::new()).is_none());
    }

    #[test]
    fn lane_label_mapping_round_trips() {
        assert_eq!(Lane::Backlog.label(), "backlog");
        assert_eq!(Lane::Burning.label(), "burning");
        assert_eq!(Lane::Tantra.label(), "tantra");
        assert_eq!(Lane::Done.label(), "done");

        assert_eq!(Lane::parse("burning"), Some(Lane::Burning));
        assert_eq!(Lane::parse("BURNING"), Some(Lane::Burning));
        assert_eq!(Lane::parse("Done"), Some(Lane::Done));
        assert_eq!(Lane::parse("wip"), None);
        assert_eq!(Lane::parse(""), None);
    }

    #[test]
    fn todo_title_convention_prefixes_login() {
        assert_eq!(todo_title("8bit-wraith"), "todo: 8bit-wraith");
        assert_eq!(todo_title("peterlodri-sec"), "todo: peterlodri-sec");
        assert_eq!(daily_title("standardgalactic"), "daily: standardgalactic");
    }

    #[test]
    fn find_by_title_prefix_matches_first_open_issue() {
        let issues = vec![
            test_issue(1, "todo: peterlodri-sec"),
            test_issue(2, "daily: peterlodri-sec"),
            test_issue(3, "todo: 8bit-wraith"),
        ];
        assert_eq!(
            find_by_title_prefix(&issues, "todo: ").map(|issue| issue.number),
            Some(1)
        );
        assert_eq!(
            find_by_title_prefix(&issues, "daily: ").map(|issue| issue.number),
            Some(2)
        );
        assert_eq!(
            find_by_title_prefix(&issues, "done: ").map(|issue| issue.number),
            None
        );
    }

    #[test]
    fn lane_label_of_issue_returns_first_matching_lane() {
        let card = GhIssue {
            number: 5,
            title: "BRIDGE Phase 2".into(),
            state: "open".into(),
            labels: vec![
                GhLabel {
                    name: "enhancement".into(),
                },
                GhLabel {
                    name: "burning".into(),
                },
            ],
            body: None,
            html_url: None,
            pull_request: None,
        };
        assert_eq!(card.lane_label(), Some("burning"));

        let scratch = GhIssue {
            number: 6,
            title: "todo: peterlodri-sec".into(),
            state: "open".into(),
            labels: vec![],
            body: None,
            html_url: None,
            pull_request: None,
        };
        assert_eq!(scratch.lane_label(), None);
    }

    #[test]
    fn collaborator_table_maps_env_to_login() {
        assert_eq!(COLLABORATORS[0], ("TANTRIC_TOKEN_PETER", "peterlodri-sec"));
        assert_eq!(COLLABORATORS[1], ("TANTRIC_TOKEN_8BIT", "8bit-wraith"));
        assert_eq!(COLLABORATORS[2], ("TANTRIC_TOKEN_SG", "standardgalactic"));
    }

    fn test_issue(number: u64, title: &str) -> GhIssue {
        GhIssue {
            number,
            title: title.into(),
            state: "open".into(),
            labels: vec![],
            body: None,
            html_url: None,
            pull_request: None,
        }
    }
}
