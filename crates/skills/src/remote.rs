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
}
