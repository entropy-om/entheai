//! Backer activation + beta entitlement gate.
//!
//! Credentials live at `~/.config/entheai/backer.json` — only the SHA-256 hash
//! of the license key is stored, never the raw key. `entheai activate <KEY>`
//! verifies the key against the license endpoint and persists the credential;
//! `--beta` gates on the stored entitlements.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

/// Backer credential file schema (`~/.config/entheai/backer.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackerCredential {
    pub version: u32,
    /// SHA-256 hex digest of the *normalized* license key — never the raw key.
    pub key_hash: String,
    pub entitlements: Vec<String>,
    pub email: String,
    /// Unix timestamp (milliseconds) of activation.
    pub activated_at: u64,
}

/// License verification endpoint used by `entheai activate <KEY>`.
const VERIFY_URL: &str = "https://entheai.com/api/license/verify";
/// Backer signup URL printed when activation fails.
const BACKER_URL: &str = "https://entheai.com/back";

/// Credential file path for a given home dir: `<home>/.config/entheai/backer.json`.
pub fn credential_path_from_home(home: &Path) -> PathBuf {
    home.join(".config").join("entheai").join("backer.json")
}

/// Credential file path, resolved via `$HOME`. `None` when `HOME` is unset.
pub fn credential_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| credential_path_from_home(&home))
}

/// Normalize a license key: trim surrounding whitespace, then uppercase.
pub fn normalize_key(key: &str) -> String {
    key.trim().to_uppercase()
}

/// Hex-encode a byte slice (lowercase).
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// SHA-256 hex digest of a (normalized) license key.
pub fn key_hash_hex(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    to_hex(&hasher.finalize())
}

/// True when the credential carries the given entitlement.
pub fn has_entitlement(cred: &BackerCredential, entitlement: &str) -> bool {
    cred.entitlements.iter().any(|e| e == entitlement)
}

/// Load `~/.config/entheai/backer.json`. A missing file → `Ok(None)`; a corrupt
/// file is a hard error (never silently downgrade a paid credential).
pub fn load_credential() -> anyhow::Result<Option<BackerCredential>> {
    let Some(path) = credential_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let cred: BackerCredential =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(cred))
}

/// Persist the credential to `~/.config/entheai/backer.json` (creates parent
/// dirs; write failures surface as errors — a lost credential would silently
/// downgrade the user).
pub fn write_credential(cred: &BackerCredential) -> anyhow::Result<()> {
    let path = credential_path().context("HOME is unset; cannot locate ~/.config/entheai")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(cred)?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Response shape of the license verification endpoint.
#[derive(Debug, Deserialize)]
struct VerifyResponse {
    ok: bool,
}

/// Print the one-line failure + become-a-backer hint, and return the error
/// that makes `main` exit 1.
fn fail_activation(msg: impl std::fmt::Display) -> anyhow::Error {
    eprintln!("{msg}");
    eprintln!("become a backer → {BACKER_URL}");
    anyhow::anyhow!("{msg}")
}

/// `entheai activate <KEY>`: verify the key against the license endpoint, then
/// persist the credential and print a success line. Any failure prints a
/// one-line error + become-a-backer hint and returns `Err` (exit 1).
pub async fn activate(key: &str) -> anyhow::Result<()> {
    let normalized = normalize_key(key);
    if normalized.is_empty() {
        return Err(fail_activation("no key provided"));
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("entheai/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building HTTP client")?;
    let resp = client
        .post(VERIFY_URL)
        .json(&serde_json::json!({ "key": normalized }))
        .send()
        .await
        .with_context(|| format!("verifying key at {VERIFY_URL}"))?;
    if !resp.status().is_success() {
        return Err(fail_activation(format!(
            "license server rejected the key (HTTP {})",
            resp.status()
        )));
    }
    let body: VerifyResponse = resp.json().await.context("parsing license response")?;
    if !body.ok {
        return Err(fail_activation("license server rejected the key"));
    }
    let cred = BackerCredential {
        version: 1,
        key_hash: key_hash_hex(&normalized),
        entitlements: vec!["beta".to_string()],
        email: String::new(),
        activated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    };
    write_credential(&cred)?;
    println!("backer activated — beta channel unlocked");
    Ok(())
}

/// `--beta` gate: verify a stored backer credential carries the `beta`
/// entitlement. Prints a short confirmation on success; otherwise prints
/// `beta requires a backer key — run 'entheai activate <KEY>'` and returns
/// `Err` (exit 1). A corrupt credential file is a hard error.
pub fn ensure_beta(enabled: bool) -> anyhow::Result<()> {
    if !enabled {
        return Ok(());
    }
    match load_credential() {
        Ok(Some(cred)) if has_entitlement(&cred, "beta") => {
            println!("beta channel unlocked — welcome backer");
            Ok(())
        }
        Ok(_) => {
            eprintln!("beta requires a backer key — run 'entheai activate <KEY>'");
            bail!("beta requires a backer key — run 'entheai activate <KEY>'")
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_key_by_trimming_and_uppercasing() {
        assert_eq!(normalize_key("  abc-def-123  "), "ABC-DEF-123");
        assert_eq!(normalize_key("abc"), "ABC");
        assert_eq!(normalize_key("ABC"), "ABC");
        assert_eq!(normalize_key("  "), "");
    }

    #[test]
    fn key_hash_is_lowercase_hex_sha256() {
        // sha256("ABC") — `printf ABC | shasum -a 256`.
        let digest = key_hash_hex("ABC");
        assert_eq!(
            digest,
            "b5d4045c3f466fa91fe2cc6abe79232a1a57cdf104f7a26e716e0a1e2789df78"
        );
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(digest.to_lowercase(), digest, "hex digest is lowercase");
    }

    #[test]
    fn credential_path_ends_with_entheai_backer_json() {
        let p = credential_path_from_home(Path::new("/home/backer"));
        assert!(p.ends_with("entheai/backer.json"));
        assert_eq!(p, PathBuf::from("/home/backer/.config/entheai/backer.json"));
    }

    #[test]
    fn entitlement_check_matches_exact_names() {
        let cred = BackerCredential {
            version: 1,
            key_hash: "deadbeef".to_string(),
            entitlements: vec!["beta".to_string()],
            email: "backer@example.com".to_string(),
            activated_at: 42,
        };
        assert!(has_entitlement(&cred, "beta"));
        assert!(!has_entitlement(&cred, "pro"));
        assert!(!has_entitlement(&cred, "BETA"));
        assert!(!has_entitlement(&cred, ""));
    }

    #[test]
    fn credential_round_trips_through_json() {
        let cred = BackerCredential {
            version: 1,
            key_hash: key_hash_hex("ABC"),
            entitlements: vec!["beta".to_string()],
            email: "backer@example.com".to_string(),
            activated_at: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&cred).unwrap();
        let back: BackerCredential = serde_json::from_str(&json).unwrap();
        assert_eq!(cred, back);
    }
}
