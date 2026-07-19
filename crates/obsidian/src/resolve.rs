//! Vault resolution (spec §4). Detection only — no writes.

use std::path::{Path, PathBuf};

/// Resolve the vault directory for `repo_root`, honoring an explicit override.
/// Rules (first hit wins): (1) `vault_path_override` if non-empty and a valid
/// vault; (2) `~/Library/Mobile Documents/iCloud~md~obsidian/<repo-name>` if a
/// valid vault; (3) None. A "valid vault" is a directory containing `.obsidian/`.
/// `home` is injected for testability (the bin passes the real `$HOME`).
pub fn resolve_vault(repo_root: &Path, vault_path_override: &str, home: &Path) -> Option<PathBuf> {
    if !vault_path_override.is_empty() {
        let p = expand_home(vault_path_override, home);
        return is_vault(&p).then_some(p);
    }
    let name = repo_root.file_name()?;
    let candidate = home
        .join("Library/Mobile Documents/iCloud~md~obsidian")
        .join(name);
    is_vault(&candidate).then_some(candidate)
}

/// A directory is a vault iff it contains a `.obsidian/` subdirectory.
fn is_vault(dir: &Path) -> bool {
    dir.join(".obsidian").is_dir()
}

fn expand_home(path: &str, home: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vault(at: &Path) {
        std::fs::create_dir_all(at.join(".obsidian")).unwrap();
    }

    #[test]
    fn explicit_override_wins_when_valid() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("myvault");
        make_vault(&vault);
        let got = resolve_vault(
            Path::new("/whatever/repo"),
            vault.to_str().unwrap(),
            dir.path(),
        );
        assert_eq!(got.as_deref(), Some(vault.as_path()));
    }

    #[test]
    fn autodetects_icloud_vault_by_repo_name() {
        let home = tempfile::tempdir().unwrap();
        let vault = home
            .path()
            .join("Library/Mobile Documents/iCloud~md~obsidian/entheai");
        make_vault(&vault);
        let got = resolve_vault(Path::new("/x/entheai"), "", home.path());
        assert_eq!(got.as_deref(), Some(vault.as_path()));
    }

    #[test]
    fn same_named_dir_without_dot_obsidian_is_not_a_vault() {
        let home = tempfile::tempdir().unwrap();
        // Directory exists but has no `.obsidian/` → not a vault.
        std::fs::create_dir_all(
            home.path()
                .join("Library/Mobile Documents/iCloud~md~obsidian/entheai"),
        )
        .unwrap();
        assert!(resolve_vault(Path::new("/x/entheai"), "", home.path()).is_none());
    }

    #[test]
    fn none_when_nothing_resolves() {
        let home = tempfile::tempdir().unwrap();
        assert!(resolve_vault(Path::new("/x/entheai"), "", home.path()).is_none());
    }
}
