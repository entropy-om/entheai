//! Add a skill from a URL via layered well-known discovery:
//! `GET /.well-known/skills.json` (entheai-native manifest) → `GET /llms.txt`
//! (docs convention) → `GET <url>` (last-resort page extract). Each result is
//! written as `skills/<slug>/SKILL.md`, which `SkillRegistry::discover` finds.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

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
/// Cap on manifest entries processed — a single `add` shouldn't fan out to an
/// unbounded number of fetches/writes from a hostile manifest.
const MAX_MANIFEST_SKILLS: usize = 64;

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
        anyhow::bail!(
            "refusing to write outside the skills dir: {}",
            skill_dir.display()
        );
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
    // Sanitize frontmatter values: a remote-controlled `name`/`description` with a
    // newline could otherwise inject extra frontmatter lines.
    let one_line = |s: &str| s.replace(['\n', '\r'], " ");
    let doc = format!(
        "---\nname: {}\ndescription: {}\nsource: {}\n---\n\n{body}\n",
        one_line(name),
        one_line(description),
        one_line(source),
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
    // Rust's `regex` crate has no backreferences, so match each tag pair
    // explicitly instead of the backreferenced `<(script|style)>...</\1>` form.
    let drop_blocks =
        regex::Regex::new(r"(?is)<script\b.*?</\s*script\s*>|<style\b.*?</\s*style\s*>").unwrap();
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

/// Fetch + parse a manifest entry's `skill_md_url`. Restricted to the manifest's
/// own host (an SSRF guard: a site's manifest may only reference its own SKILL.md
/// files, never an internal address or a third-party host) and http/https, with a
/// success-status check so an error page is never written as a skill body.
async fn fetch_skill_md(
    client: &reqwest::Client,
    base: &reqwest::Url,
    entry: &ManifestSkill,
    skill_md_url: &str,
) -> anyhow::Result<(String, String)> {
    let u = reqwest::Url::parse(skill_md_url)
        .map_err(|e| anyhow::anyhow!("bad skill_md_url {skill_md_url:?}: {e}"))?;
    if !matches!(u.scheme(), "http" | "https") {
        anyhow::bail!("skill_md_url must be http/https (got {:?})", u.scheme());
    }
    if u.host_str() != base.host_str() {
        anyhow::bail!(
            "skill_md_url host {:?} differs from the manifest host {:?} — cross-origin not allowed",
            u.host_str(),
            base.host_str()
        );
    }
    let resp = client.get(u).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("skill_md_url returned HTTP {}", resp.status());
    }
    let md = read_capped(resp, BODY_CAP).await?;
    let (_n, d, b) = crate::parse_skill_md(&md, &entry.name);
    let desc = if entry.description.is_empty() {
        d
    } else {
        entry.description.clone()
    };
    Ok((desc, b))
}

/// Fetch a skill (or skills) from `url` via layered discovery and write them
/// under `skills_dir`. Returns what was written (incl. skip-if-exists). Errors
/// only on a bad URL or when no tier yields anything.
pub async fn add_from_url(url: &str, skills_dir: &Path) -> anyhow::Result<Vec<AddedSkill>> {
    let parsed =
        reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL {url:?}: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!(
            "only http/https URLs are supported (got {:?})",
            parsed.scheme()
        );
    }
    let host = parsed.host_str().unwrap_or("skill").to_string();
    let client = reqwest::Client::builder()
        .timeout(REQ_TIMEOUT)
        // SSRF guard: follow redirects ONLY within the same host. Without this,
        // reqwest follows up to 10 redirects to any host, so a same-host manifest
        // could 302 a `skill_md_url` to an internal address (e.g. the cloud
        // metadata endpoint 169.254.169.254) and that response would be written
        // as a skill. Same-host redirects (http→https, path normalization) still
        // work; a cross-host redirect is stopped (non-2xx → the fetch is skipped).
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let prev_host = attempt.previous().last().and_then(|u| u.host_str());
            if prev_host.is_some() && prev_host == attempt.url().host_str() {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()?;

    // Tier 1: entheai-native manifest.
    let manifest_url = parsed.join("/.well-known/skills.json")?;
    if let Ok(resp) = client.get(manifest_url.clone()).send().await {
        if resp.status().is_success() {
            if let Ok(text) = read_capped(resp, BODY_CAP).await {
                if let Ok(manifest) = serde_json::from_str::<Manifest>(&text) {
                    if manifest.skills.len() > MAX_MANIFEST_SKILLS {
                        log::warn!(
                            "skills: manifest lists {} entries — only the first {} are processed",
                            manifest.skills.len(),
                            MAX_MANIFEST_SKILLS
                        );
                    }
                    let mut added = Vec::new();
                    // A single bad entry (missing fields, unreachable/error/cross-origin
                    // skill_md_url, unslugifiable name) is skipped with a warning rather
                    // than aborting the batch and orphaning earlier writes.
                    for s in manifest.skills.iter().take(MAX_MANIFEST_SKILLS) {
                        let (desc, body) = match (&s.body, &s.skill_md_url) {
                            (Some(b), _) => (s.description.clone(), b.clone()),
                            (None, Some(u)) => match fetch_skill_md(&client, &parsed, s, u).await {
                                Ok(v) => v,
                                Err(e) => {
                                    log::warn!(
                                        "skills: manifest entry {:?}: {e} — skipping",
                                        s.name
                                    );
                                    continue;
                                }
                            },
                            (None, None) => {
                                log::warn!("skills: manifest entry {:?} has neither body nor skill_md_url — skipping", s.name);
                                continue;
                            }
                        };
                        match write_skill(skills_dir, &s.name, &desc, &body, url) {
                            Ok(mut a) => {
                                a.tier = "well-known";
                                added.push(a);
                            }
                            Err(e) => {
                                log::warn!("skills: manifest entry {:?}: {e} — skipping", s.name);
                                continue;
                            }
                        }
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
                    let (name, desc, body) =
                        synthesize_from_llms_txt(&text, &host, llms_url.as_str());
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
                    format!(
                        "> auto-extracted from {url} (best-effort, may be noisy).\n\n{}",
                        html_to_text(&text)
                    )
                } else {
                    format!("> Added from {url}.\n\n{text}")
                };
                let mut a =
                    write_skill(skills_dir, &host, &format!("Docs from {host}"), &body, url)?;
                a.tier = "page";
                return Ok(vec![a]);
            }
        }
    }

    anyhow::bail!(
        "no skill found at {url} (tried {manifest_url}, {llms_url}, and the page itself)"
    );
}

/// Remove an installed skill directory by name. The name is slugified the same
/// way `add` slugified it (so `remove` is traversal-safe: a `../..` name resolves
/// to a harmless in-dir slug that simply won't exist). Returns the removed path,
/// or `None` if no such skill dir exists.
pub fn remove_from_dir(skills_dir: &Path, name: &str) -> anyhow::Result<Option<PathBuf>> {
    let slug = slugify(name)?;
    let skill_dir = skills_dir.join(&slug);
    if skill_dir.parent() != Some(skills_dir) {
        anyhow::bail!(
            "refusing to remove outside the skills dir: {}",
            skill_dir.display()
        );
    }
    if !skill_dir.exists() {
        return Ok(None);
    }
    std::fs::remove_dir_all(&skill_dir)?;
    Ok(Some(skill_dir))
}

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

    #[test]
    fn write_skill_creates_file_and_skips_existing() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_skill(
            dir.path(),
            "My Skill",
            "desc here",
            "body text",
            "https://x.example",
        )
        .unwrap();
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

    #[test]
    fn synthesize_from_llms_txt_extracts_title_and_blurb() {
        let txt = "# Stripe Docs\n\n> Payments infrastructure for the internet.\n\n## Guides\n- [Quickstart](/q)\n";
        let (name, desc, body) =
            synthesize_from_llms_txt(txt, "docs.stripe.com", "https://docs.stripe.com/llms.txt");
        assert_eq!(name, "Stripe Docs");
        assert_eq!(desc, "Payments infrastructure for the internet.");
        assert!(body.contains("https://docs.stripe.com/llms.txt")); // source note
        assert!(body.contains("Quickstart")); // original index retained
    }

    #[test]
    fn synthesize_falls_back_to_host_and_first_paragraph() {
        let txt = "No heading here.\nSecond line.\n";
        let (name, desc, _body) =
            synthesize_from_llms_txt(txt, "example.com", "https://example.com/llms.txt");
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

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn tier1_manifest_wins_and_installs_all_skills() {
        let server = MockServer::start().await;
        let manifest = r#"{"skills":[
            {"name":"Alpha","description":"a","body":"do alpha"},
            {"name":"Beta","description":"b","skill_md_url":"__BASE__/beta.md"}
        ]}"#
        .replace("__BASE__", &server.uri());
        Mock::given(method("GET"))
            .and(path("/.well-known/skills.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(manifest))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/beta.md"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("---\nname: Beta\ndescription: b\n---\ndo beta"),
            )
            .mount(&server)
            .await;
        // llms.txt also present — must be ignored because tier 1 matched.
        Mock::given(method("GET"))
            .and(path("/llms.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("# Should Not Win"))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let added = add_from_url(&server.uri(), dir.path()).await.unwrap();
        assert_eq!(added.len(), 2);
        assert!(added.iter().all(|a| a.tier == "well-known"));
        assert!(dir.path().join("alpha/SKILL.md").exists());
        assert!(dir.path().join("beta/SKILL.md").exists());
        assert!(std::fs::read_to_string(dir.path().join("beta/SKILL.md"))
            .unwrap()
            .contains("do beta"));
    }

    #[tokio::test]
    async fn tier1_skips_bad_entry_installs_good_and_never_writes_error_page() {
        let server = MockServer::start().await;
        let manifest = r#"{"skills":[
            {"name":"Good","description":"g","body":"do good"},
            {"name":"Bad","description":"b","skill_md_url":"__BASE__/missing.md"}
        ]}"#
        .replace("__BASE__", &server.uri());
        Mock::given(method("GET"))
            .and(path("/.well-known/skills.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(manifest))
            .mount(&server)
            .await;
        // 404 with an HTML error page — must NOT be written as Bad's instructions.
        Mock::given(method("GET"))
            .and(path("/missing.md"))
            .respond_with(
                ResponseTemplate::new(404).set_body_raw("<h1>not found</h1>", "text/html"),
            )
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let added = add_from_url(&server.uri(), dir.path()).await.unwrap();
        assert_eq!(added.len(), 1, "only the good entry installs");
        assert_eq!(added[0].slug, "good");
        assert!(dir.path().join("good/SKILL.md").exists());
        assert!(
            !dir.path().join("bad/SKILL.md").exists(),
            "the 404 page must not become a skill"
        );
    }

    #[tokio::test]
    async fn tier1_rejects_cross_origin_skill_md_url() {
        let server = MockServer::start().await;
        // skill_md_url points at a different host → rejected; no manifest skill
        // installed → falls through to llms.txt.
        let manifest = r#"{"skills":[{"name":"X","description":"x","skill_md_url":"http://169.254.169.254/latest/meta-data"}]}"#;
        Mock::given(method("GET"))
            .and(path("/.well-known/skills.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(manifest))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/llms.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("# Fell Through\n\n> ok"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let added = add_from_url(&server.uri(), dir.path()).await.unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].tier, "llms.txt");
        assert_eq!(added[0].slug, "fell-through");
    }

    #[test]
    fn remove_from_dir_removes_only_the_named_skill_and_is_traversal_safe() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "Keep Me", "d", "b", "src").unwrap();
        write_skill(dir.path(), "Zap Me", "d", "b", "src").unwrap();
        let removed = remove_from_dir(dir.path(), "Zap Me").unwrap();
        assert_eq!(removed, Some(dir.path().join("zap-me")));
        assert!(!dir.path().join("zap-me").exists());
        assert!(dir.path().join("keep-me").exists()); // untouched
        assert_eq!(remove_from_dir(dir.path(), "nope").unwrap(), None); // missing → None
                                                                        // A traversal attempt slugifies to a harmless in-dir name that doesn't exist.
        assert_eq!(remove_from_dir(dir.path(), "../../etc").unwrap(), None);
    }

    #[tokio::test]
    async fn tier2_llms_txt_when_no_manifest() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/llms.txt"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("# Docs\n\n> Great docs.\n\n- [X](/x)"),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let added = add_from_url(&server.uri(), dir.path()).await.unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].tier, "llms.txt");
        assert_eq!(added[0].slug, "docs");
        assert!(std::fs::read_to_string(&added[0].path)
            .unwrap()
            .contains("Great docs."));
    }

    #[tokio::test]
    async fn tier3_page_extract_when_nothing_else() {
        let server = MockServer::start().await;
        // `set_body_string` unconditionally forces mime="text/plain" at
        // generate_response() time, overriding any `insert_header("content-type", ..)`
        // regardless of call order (wiremock 0.6.5). Use `set_body_raw(body, mime)`,
        // which sets both atomically, to actually get a `text/html` response.
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw("<h1>Hello</h1><p>World</p>", "text/html"),
            )
            .mount(&server)
            .await;
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
        assert!(add_from_url("file:///etc/passwd", dir.path())
            .await
            .is_err());
    }
}
