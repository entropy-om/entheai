# `entheai --skills add <url>` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `entheai --skills add <url>` fetches a skill from a domain via layered well-known discovery and writes it as `skills/<slug>/SKILL.md`, which `SkillRegistry::discover` then picks up.

**Architecture:** A new `crates/skills/src/remote.rs` owns fetch + discovery + synthesis + install. `add_from_url(url, skills_dir)` tries, in order: `GET <origin>/.well-known/skills.json` (entheai-native manifest) → `GET <origin>/llms.txt` (docs convention; Stripe serves it) → `GET <url>` (last-resort page extract). Each produced skill is slugified (strict `[a-z0-9-]+`, path-traversal-safe), written with provenance frontmatter, skipped if it already exists. The bin gets a `--skills` flag-verb that runs before the interactive path (like `--memory`/`--doctor`).

**Tech Stack:** Rust (edition 2021, MSRV 1.80). `reqwest` (workspace, rustls — no system libs), `serde`/`serde_json`, `tokio`, `anyhow`, `regex`; `wiremock` (workspace) + `tempfile` for hermetic HTTP tests.

**Spec:** `docs/superpowers/specs/2026-07-20-skills-add-url-design.md`.

**Verified API facts (do not deviate):**
- `reqwest::Url::parse(s)?`; `url.scheme()` (reject non `http`/`https`); `url.join("/llms.txt")?` resolves an origin-rooted path regardless of the input path (so it derives the origin for us).
- `reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?`; `client.get(u).send().await?`; `resp.status().is_success()`; `resp.content_length() -> Option<u64>`; `resp.headers().get(reqwest::header::CONTENT_TYPE)`; `resp.chunk().await? -> Option<Bytes>` (needs `mut resp`) for capped streaming reads.
- The workspace `reqwest` is `default-features=false, features=["json","stream","rustls-tls"]` — `.chunk()` needs `stream` (present). We read+cap manually and parse with `serde_json` (not `resp.json`) to bound memory.
- `wiremock`: `MockServer::start().await`; `Mock::given(method("GET")).and(path("/llms.txt")).respond_with(ResponseTemplate::new(200).set_body_string(s)).mount(&server).await`; `server.uri()` → `http://127.0.0.1:<port>`. Unmatched paths return 404.

**Key seams (verified):**
- `crates/skills/src/lib.rs::parse_skill_md(text, fallback) -> (name, description, body)` — reuse for `skill_md_url` fetches.
- `crates/skills/src/lib.rs::SkillRegistry::discover` scans `[skills].dirs` sub-dirs for `SKILL.md`.
- `crates/config`: `cfg.skills.dirs: Vec<String>` (default `["skills"]`).
- `bin/entheai/src/main.rs`: CLI struct `Cli` (~line 19, all flags); early-exit handlers `--doctor`/`--memory` short-circuit before `setup_companion`/`match cli.prompt`. `dotenvy::dotenv().ok()` at ~line 49; `root` is resolved before those handlers.

---

## File Structure

- **Create `crates/skills/src/remote.rs`** — the whole feature: `AddedSkill`, `Manifest`/`ManifestSkill`, `add_from_url`, `slugify`, `synthesize_from_llms_txt`, `html_to_text`, `read_capped`, `write_skill`.
- **Modify `crates/skills/src/lib.rs`** — add `pub mod remote;`.
- **Modify `crates/skills/Cargo.toml`** — deps + dev-deps.
- **Modify `bin/entheai/src/main.rs`** — `--skills` arg + `run_skills_cmd` + early dispatch.

---

## Task 1: Add deps + module skeleton

**Files:** Modify `crates/skills/Cargo.toml`; Create `crates/skills/src/remote.rs`; Modify `crates/skills/src/lib.rs`.

- [ ] **Step 1: Cargo.toml deps**

Replace the `[dependencies]`/`[dev-dependencies]` of `crates/skills/Cargo.toml` with:

```toml
[dependencies]
entheai-tools = { path = "../tools" }
serde_json.workspace = true
async-trait.workspace = true
serde = { workspace = true }
anyhow = { workspace = true }
reqwest = { workspace = true }
tokio = { workspace = true, features = ["rt-multi-thread", "macros"] }
regex = "1"

[dev-dependencies]
tempfile = "3"
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
wiremock = { workspace = true }
```

- [ ] **Step 2: Declare the module**

In `crates/skills/src/lib.rs`, add after the crate doc-comment / first `use` block:

```rust
pub mod remote;
```

- [ ] **Step 3: Skeleton `remote.rs` (compiles, no logic yet)**

```rust
//! Add a skill from a URL via layered well-known discovery:
//! `GET /.well-known/skills.json` (entheai-native manifest) → `GET /llms.txt`
//! (docs convention) → `GET <url>` (last-resort page extract). Each result is
//! written as `skills/<slug>/SKILL.md`, which `SkillRegistry::discover` finds.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// One skill written to disk by `add_from_url`.
#[derive(Debug, Clone, PartialEq)]
pub struct AddedSkill {
    pub name: String,
    pub slug: String,
    pub path: PathBuf,
    pub source: String,
    pub tier: &'static str,
    pub skipped_existing: bool,
}

const BODY_CAP: usize = 1024 * 1024; // 1 MiB
const REQ_TIMEOUT: Duration = Duration::from_secs(15);
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p entheai-skills`
Expected: PASS (unused-const warnings are fine at this stage).

- [ ] **Step 5: Commit**

```bash
git add crates/skills/Cargo.toml crates/skills/src/lib.rs crates/skills/src/remote.rs Cargo.lock
git commit -m "chore(skills): scaffold remote (--skills add) module + deps"
```

---

## Task 2: `slugify` (path-traversal-safe)

**Files:** Modify `crates/skills/src/remote.rs`.

- [ ] **Step 1: Failing test**

Append to `remote.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_makes_safe_filenames() {
        assert_eq!(slugify("Stripe Payments").unwrap(), "stripe-payments");
        assert_eq!(slugify("docs.stripe.com").unwrap(), "docs-stripe-com");
        // Path-traversal attempts must not survive as path separators or dots.
        let s = slugify("../../etc/passwd").unwrap();
        assert!(!s.contains('/') && !s.contains('.') && !s.contains(".."));
        assert_eq!(s, "etc-passwd");
        assert_eq!(slugify("  Hello__World!!  ").unwrap(), "hello-world");
        assert!(slugify("   ").is_err()); // empty after slugging
        assert!(slugify("!!!").is_err());
        // Length cap.
        assert!(slugify(&"a".repeat(200)).unwrap().len() <= 64);
    }
}
```

- [ ] **Step 2: Run — expect FAIL** (`slugify` undefined)

Run: `cargo test -p entheai-skills slugify -- --color=never`

- [ ] **Step 3: Implement `slugify`** (add above the `#[cfg(test)]`):

```rust
/// Slugify to a safe directory name: lowercase, non-`[a-z0-9]` runs → `-`,
/// trimmed, collapsed, capped at 64. Structurally strips `/`, `.`, `..`, so a
/// remote-controlled `name` cannot escape the skills dir. Errors if empty.
fn slugify(name: &str) -> anyhow::Result<String> {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let slug = out.trim_matches('-').to_string();
    let slug: String = slug.chars().take(64).collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        anyhow::bail!("cannot derive a valid skill name from {name:?}");
    }
    Ok(slug)
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p entheai-skills slugify -- --color=never`

- [ ] **Step 5: Commit**

```bash
git add crates/skills/src/remote.rs
git commit -m "feat(skills): path-traversal-safe slugify for --skills add"
```

---

## Task 3: `read_capped` + `write_skill` (bounded read, skip-if-exists)

**Files:** Modify `crates/skills/src/remote.rs`.

- [ ] **Step 1: Failing test** (append inside `mod tests`):

```rust
    #[test]
    fn write_skill_creates_file_and_skips_existing() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_skill(dir.path(), "My Skill", "desc here", "body text", "https://x.example").unwrap();
        assert_eq!(a.slug, "my-skill");
        assert!(!a.skipped_existing);
        let text = std::fs::read_to_string(&a.path).unwrap();
        assert!(text.contains("name: My Skill"));
        assert!(text.contains("description: desc here"));
        assert!(text.contains("source: https://x.example"));
        assert!(text.contains("body text"));
        // Re-add → skip, no clobber.
        std::fs::write(&a.path, "EDITED").unwrap();
        let b = write_skill(dir.path(), "My Skill", "d2", "b2", "https://x.example").unwrap();
        assert!(b.skipped_existing);
        assert_eq!(std::fs::read_to_string(&b.path).unwrap(), "EDITED");
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p entheai-skills write_skill -- --color=never`

- [ ] **Step 3: Implement `read_capped` + `write_skill`** (add above `#[cfg(test)]`):

```rust
/// Read a response body, bounded to `cap` bytes (reject over-large via
/// Content-Length when present, else stream-cap). Lossy UTF-8.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> anyhow::Result<String> {
    if let Some(len) = resp.content_length() {
        if len as usize > cap {
            anyhow::bail!("response too large ({len} bytes > {cap} cap)");
        }
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > cap {
            anyhow::bail!("response exceeded {cap}-byte cap");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Write `skills_dir/<slug>/SKILL.md` with provenance frontmatter. Skip-if-exists:
/// if the skill dir already exists, return it flagged and do not clobber.
fn write_skill(
    skills_dir: &Path,
    name: &str,
    description: &str,
    body: &str,
    source: &str,
) -> anyhow::Result<AddedSkill> {
    let slug = slugify(name)?;
    let skill_dir = skills_dir.join(&slug);
    // Defense in depth: the joined path must stay inside skills_dir.
    if skill_dir.parent() != Some(skills_dir) {
        anyhow::bail!("refusing to write outside the skills dir: {}", skill_dir.display());
    }
    let path = skill_dir.join("SKILL.md");
    if skill_dir.exists() {
        return Ok(AddedSkill {
            name: name.to_string(),
            slug,
            path,
            source: source.to_string(),
            tier: "",
            skipped_existing: true,
        });
    }
    std::fs::create_dir_all(&skill_dir)?;
    let doc = format!(
        "---\nname: {name}\ndescription: {description}\nsource: {source}\n---\n\n{body}\n"
    );
    std::fs::write(&path, doc)?;
    Ok(AddedSkill {
        name: name.to_string(),
        slug,
        path,
        source: source.to_string(),
        tier: "",
        skipped_existing: false,
    })
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p entheai-skills write_skill -- --color=never`

- [ ] **Step 5: Commit**

```bash
git add crates/skills/src/remote.rs
git commit -m "feat(skills): bounded read_capped + skip-if-exists write_skill"
```

---

## Task 4: llms.txt synthesis + HTML→text

**Files:** Modify `crates/skills/src/remote.rs`.

- [ ] **Step 1: Failing tests** (append inside `mod tests`):

```rust
    #[test]
    fn synthesize_from_llms_txt_extracts_title_and_blurb() {
        let txt = "# Stripe Docs\n\n> Payments infrastructure for the internet.\n\n## Guides\n- [Quickstart](/q)\n";
        let (name, desc, body) = synthesize_from_llms_txt(txt, "docs.stripe.com", "https://docs.stripe.com/llms.txt");
        assert_eq!(name, "Stripe Docs");
        assert_eq!(desc, "Payments infrastructure for the internet.");
        assert!(body.contains("https://docs.stripe.com/llms.txt")); // source note
        assert!(body.contains("Quickstart"));                        // original index retained
    }

    #[test]
    fn synthesize_falls_back_to_host_and_first_paragraph() {
        let txt = "No heading here.\nSecond line.\n";
        let (name, desc, _body) = synthesize_from_llms_txt(txt, "example.com", "https://example.com/llms.txt");
        assert_eq!(name, "example.com");
        assert_eq!(desc, "No heading here.");
    }

    #[test]
    fn html_to_text_strips_tags_scripts_styles() {
        let html = "<html><head><style>x{}</style></head><body><script>evil()</script><h1>Hi</h1><p>A &amp; B</p></body></html>";
        let out = html_to_text(html);
        assert!(!out.contains('<') && !out.contains("evil"));
        assert!(out.contains("Hi") && out.contains("A & B"));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p entheai-skills -- --color=never synthesize html_to_text`

- [ ] **Step 3: Implement** (add above `#[cfg(test)]`):

```rust
/// Build (name, description, body) from an `llms.txt`. `name` = first `# ` heading,
/// else `host`. `description` = first `>` blockquote line, else first non-heading
/// non-empty line (≤200 chars). `body` = the file, prefixed with a source note.
fn synthesize_from_llms_txt(txt: &str, host: &str, source: &str) -> (String, String, String) {
    let mut name = host.to_string();
    let mut blockquote: Option<String> = None;
    let mut first_para: Option<String> = None;
    for line in txt.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(h) = t.strip_prefix("# ") {
            if name == host {
                name = h.trim().to_string();
            }
        } else if let Some(q) = t.strip_prefix('>') {
            blockquote.get_or_insert_with(|| q.trim().to_string());
        } else if !t.starts_with('#') {
            first_para.get_or_insert_with(|| t.to_string());
        }
    }
    let mut description = blockquote.or(first_para).unwrap_or_default();
    if description.len() > 200 {
        description.truncate(200);
    }
    let body = format!(
        "> Skill added from {source} (an llms.txt docs index). Full text may be at the site's /llms-full.txt.\n\n{txt}"
    );
    (name, description, body)
}

/// Best-effort HTML→text: drop `<script>`/`<style>` blocks, strip tags, decode a
/// few entities, collapse whitespace. Noisy by nature — callers label it.
fn html_to_text(html: &str) -> String {
    let drop_blocks = regex::Regex::new(r"(?is)<(script|style)\b.*?</\s*\1\s*>").unwrap();
    let no_blocks = drop_blocks.replace_all(html, " ");
    let tags = regex::Regex::new(r"(?s)<[^>]*>").unwrap();
    let no_tags = tags.replace_all(&no_blocks, " ");
    let decoded = no_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p entheai-skills -- --color=never synthesize html_to_text`

- [ ] **Step 5: Commit**

```bash
git add crates/skills/src/remote.rs
git commit -m "feat(skills): llms.txt synthesis + best-effort html_to_text"
```

---

## Task 5: `add_from_url` orchestration (the 3 tiers) + manifest types

**Files:** Modify `crates/skills/src/remote.rs`.

- [ ] **Step 1: Failing tests (hermetic via wiremock)** (append inside `mod tests`):

```rust
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn tier1_manifest_wins_and_installs_all_skills() {
        let server = MockServer::start().await;
        let manifest = r#"{"skills":[
            {"name":"Alpha","description":"a","body":"do alpha"},
            {"name":"Beta","description":"b","skill_md_url":"__BASE__/beta.md"}
        ]}"#.replace("__BASE__", &server.uri());
        Mock::given(method("GET")).and(path("/.well-known/skills.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(manifest))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/beta.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("---\nname: Beta\ndescription: b\n---\ndo beta"))
            .mount(&server).await;
        // llms.txt also present — must be ignored because tier 1 matched.
        Mock::given(method("GET")).and(path("/llms.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("# Should Not Win"))
            .mount(&server).await;

        let dir = tempfile::tempdir().unwrap();
        let added = add_from_url(&server.uri(), dir.path()).await.unwrap();
        assert_eq!(added.len(), 2);
        assert!(added.iter().all(|a| a.tier == "well-known"));
        assert!(dir.path().join("alpha/SKILL.md").exists());
        assert!(dir.path().join("beta/SKILL.md").exists());
        assert!(std::fs::read_to_string(dir.path().join("beta/SKILL.md")).unwrap().contains("do beta"));
    }

    #[tokio::test]
    async fn tier2_llms_txt_when_no_manifest() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/llms.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("# Docs\n\n> Great docs.\n\n- [X](/x)"))
            .mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let added = add_from_url(&server.uri(), dir.path()).await.unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].tier, "llms.txt");
        assert_eq!(added[0].slug, "docs");
        assert!(std::fs::read_to_string(&added[0].path).unwrap().contains("Great docs."));
    }

    #[tokio::test]
    async fn tier3_page_extract_when_nothing_else() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/"))
            .respond_with(ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<h1>Hello</h1><p>World</p>"))
            .mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let added = add_from_url(&server.uri(), dir.path()).await.unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].tier, "page");
        let body = std::fs::read_to_string(&added[0].path).unwrap();
        assert!(body.contains("Hello") && body.contains("auto-extracted"));
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let dir = tempfile::tempdir().unwrap();
        assert!(add_from_url("file:///etc/passwd", dir.path()).await.is_err());
    }
```

- [ ] **Step 2: Run — expect FAIL** (`add_from_url`/`Manifest` undefined)

Run: `cargo test -p entheai-skills -- --color=never tier rejects_non_http`

- [ ] **Step 3: Implement manifest types + `add_from_url`** (add above `#[cfg(test)]`; add `use serde::Deserialize;` to the top of the file):

```rust
#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    skills: Vec<ManifestSkill>,
}

#[derive(Debug, Deserialize)]
struct ManifestSkill {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    skill_md_url: Option<String>,
}

/// Fetch a skill (or skills) from `url` via layered discovery and write them
/// under `skills_dir`. Returns what was written (incl. skip-if-exists). Errors
/// only on a bad URL or when no tier yields anything.
pub async fn add_from_url(url: &str, skills_dir: &Path) -> anyhow::Result<Vec<AddedSkill>> {
    let parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL {url:?}: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("only http/https URLs are supported (got {:?})", parsed.scheme());
    }
    let host = parsed.host_str().unwrap_or("skill").to_string();
    let client = reqwest::Client::builder().timeout(REQ_TIMEOUT).build()?;

    // Tier 1: entheai-native manifest.
    let manifest_url = parsed.join("/.well-known/skills.json")?;
    if let Ok(resp) = client.get(manifest_url.clone()).send().await {
        if resp.status().is_success() {
            if let Ok(text) = read_capped(resp, BODY_CAP).await {
                if let Ok(manifest) = serde_json::from_str::<Manifest>(&text) {
                    let mut added = Vec::new();
                    for s in &manifest.skills {
                        let (desc, body) = match (&s.body, &s.skill_md_url) {
                            (Some(b), _) => (s.description.clone(), b.clone()),
                            (None, Some(u)) => {
                                let r = client.get(u).send().await?;
                                let md = read_capped(r, BODY_CAP).await?;
                                let (_n, d, b) = crate::parse_skill_md(&md, &s.name);
                                let d = if s.description.is_empty() { d } else { s.description.clone() };
                                (d, b)
                            }
                            (None, None) => {
                                log::warn!("skills: manifest entry {:?} has neither body nor skill_md_url — skipping", s.name);
                                continue;
                            }
                        };
                        let mut a = write_skill(skills_dir, &s.name, &desc, &body, url)?;
                        a.tier = "well-known";
                        added.push(a);
                    }
                    if !added.is_empty() {
                        return Ok(added);
                    }
                }
            }
        }
    }

    // Tier 2: llms.txt.
    let llms_url = parsed.join("/llms.txt")?;
    if let Ok(resp) = client.get(llms_url.clone()).send().await {
        if resp.status().is_success() {
            if let Ok(text) = read_capped(resp, BODY_CAP).await {
                if !text.trim().is_empty() {
                    let (name, desc, body) = synthesize_from_llms_txt(&text, &host, llms_url.as_str());
                    let mut a = write_skill(skills_dir, &name, &desc, &body, url)?;
                    a.tier = "llms.txt";
                    return Ok(vec![a]);
                }
            }
        }
    }

    // Tier 3: the page itself (last resort).
    if let Ok(resp) = client.get(parsed.clone()).send().await {
        if resp.status().is_success() {
            let is_html = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|c| c.contains("html"))
                .unwrap_or(false);
            let text = read_capped(resp, BODY_CAP).await?;
            if !text.trim().is_empty() {
                let body = if is_html {
                    format!("> Auto-extracted from {url} (best-effort, may be noisy).\n\n{}", html_to_text(&text))
                } else {
                    format!("> Added from {url}.\n\n{text}")
                };
                let mut a = write_skill(skills_dir, &host, &format!("Docs from {host}"), &body, url)?;
                a.tier = "page";
                return Ok(vec![a]);
            }
        }
    }

    anyhow::bail!(
        "no skill found at {url} (tried {manifest_url}, {llms_url}, and the page itself)"
    );
}
```

Add `log = "0.4"` to `crates/skills/Cargo.toml` `[dependencies]` (the manifest warning uses it).

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p entheai-skills -- --color=never`
Expected: all tests pass (slugify, write_skill, synthesize, html_to_text, tier1/2/3, reject-scheme).

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p entheai-skills --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/skills/Cargo.toml crates/skills/src/remote.rs
git commit -m "feat(skills): add_from_url layered discovery (well-known -> llms.txt -> page)"
```

---

## Task 6: Bin wiring — `--skills add <url>`

**Files:** Modify `bin/entheai/src/main.rs`.

- [ ] **Step 1: Add the CLI arg**

In the `Cli` struct (near the `--memory` arg ~line 41), add:

```rust
    /// Add a skill from a URL, then exit: `--skills add <url>`. Discovers via
    /// <origin>/.well-known/skills.json, then /llms.txt, then the page.
    #[arg(long = "skills", num_args = 1.., value_name = "ARGS")]
    skills: Option<Vec<String>>,
```

- [ ] **Step 2: Add the handler function** (near `run_doctor_cmd`/the `--memory` handler):

```rust
/// `entheai --skills add <url>`: fetch + install a skill, then exit. Resolves the
/// install dir from `[skills].dirs` (first entry, default "skills") under `root`.
async fn run_skills_cmd(args: &[String], cfg: &entheai_config::Config, root: &std::path::Path) -> anyhow::Result<()> {
    let dir_name = cfg.skills.dirs.first().map(String::as_str).unwrap_or("skills");
    let skills_dir = root.join(dir_name);
    match args.first().map(String::as_str) {
        Some("add") if args.len() >= 2 => {
            let url = &args[1];
            let added = entheai_skills::remote::add_from_url(url, &skills_dir).await?;
            if added.is_empty() {
                println!("skills: nothing to add from {url}");
            }
            for a in &added {
                if a.skipped_existing {
                    println!("skills: {} already exists at {} — skipping (delete to re-add)", a.slug, a.path.display());
                } else {
                    println!("skills: added '{}' [{}] -> {}", a.name, a.tier, a.path.display());
                }
            }
            if added.iter().any(|a| !a.skipped_existing) {
                println!("added from {url} — a skill's instructions are advisory to the agent. It's live on the next run.");
            }
            Ok(())
        }
        _ => anyhow::bail!("usage: entheai --skills add <url>"),
    }
}
```

- [ ] **Step 3: Dispatch early (before the interactive path)**

Alongside the existing `--doctor`/`--memory` early-exit handlers in `main`, add (place it after config `cfg` and `root` are available, before `setup_companion`):

```rust
    if let Some(skills_args) = cli.skills.as_ref() {
        run_skills_cmd(skills_args, &cfg, &root).await?;
        return Ok(());
    }
```

(Match the exact surrounding style/return type of the `--memory`/`--doctor` blocks — if they `return Ok(())` inside `main`, mirror that; find the precise insertion point by reading how `--memory` is dispatched.)

- [ ] **Step 4: Build both feature configs**

Run: `cargo build -p entheai`
Expected: PASS.

Run: `cargo build -p entheai --no-default-features`
Expected: PASS (reqwest is rustls — the headless build stays clean).

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p entheai --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Smoke (usage + error path, no network)**

Run: `cargo run -q -p entheai -- --skills 2>&1 | tail -2` → prints the usage error.
Run: `cargo run -q -p entheai -- --skills bogus 2>&1 | tail -2` → usage error.

- [ ] **Step 7: Commit**

```bash
git add bin/entheai/src/main.rs
git commit -m "feat(bin): entheai --skills add <url> to install a skill from the web"
```

---

## Task 7: Full workspace gate + live smoke + docs

**Files:** Modify `entheai.toml` (doc comment); the session doc.

- [ ] **Step 1: Workspace test + clippy**

Run: `cargo test --workspace`
Expected: PASS (baseline + new skills tests).

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 2: Live smoke against a real site (this Mac has network)**

Run: `cargo run -q -p entheai -- --skills add https://docs.stripe.com`
Expected: matches **tier `llms.txt`**, writes `skills/<slug>/SKILL.md`. Verify:
`head -8 skills/*/SKILL.md` shows the frontmatter + `source: https://docs.stripe.com`.
Then **remove the throwaway skill**: `rm -rf skills/<slug>` (do not commit a fetched Stripe skill).
If Stripe's llms.txt is unavailable, try `https://developers.cloudflare.com` or note the result.

- [ ] **Step 3: Document the command**

Add a short note to `entheai.toml` near `[skills]` (or the top comment) — one line:
`# Add a skill from a URL: entheai --skills add https://docs.stripe.com` — and to the session doc.

```bash
git add entheai.toml
git commit -m "docs: mention entheai --skills add <url>"
```

---

## Self-Review

**Spec coverage:**
- ✅ CLI `--skills add <url>` flag-verb, early-exit — Task 6.
- ✅ Layered discovery well-known → llms.txt → page, first-match-wins — Task 5 (+ precedence test).
- ✅ Native manifest schema (`body` | `skill_md_url`) — Task 5.
- ✅ llms.txt synthesis (title/blurb/body) — Task 4.
- ✅ HTML last-resort, labeled — Tasks 4/5.
- ✅ Install to `skills/<slug>/`, provenance `source:` frontmatter, skip-if-exists — Task 3.
- ✅ Strict slug + path-traversal guard — Task 2 (+ the `skill_dir.parent()` assertion in Task 3).
- ✅ Bounded: 15s timeout, 1 MiB cap, http/https only, rustls — Tasks 1/3/5.
- ✅ Hermetic wiremock tests + live smoke — Tasks 5/7.

**Placeholder scan:** none — every code step is complete; the only discovery step is Task 6 Step 3's exact insertion point (bounded to "match the `--memory` dispatch site").

**Type consistency:** `AddedSkill{name,slug,path,source,tier,skipped_existing}` (Task 1) is produced by `write_skill` (Task 3), tagged in `add_from_url` (Task 5), and consumed in `run_skills_cmd` (Task 6). `slugify`→`write_skill`→`add_from_url` signatures line up. `parse_skill_md` reused for `skill_md_url` (Task 5) matches the lib.rs signature.
