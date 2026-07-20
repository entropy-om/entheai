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
