//! entheai native-app launcher: materialize the bundled shader + minimalist
//! Ghostty config, resolve the installed Ghostty, and open one branded window.

use std::path::{Path, PathBuf};

const SHADER_SRC: &str = include_str!("../assets/rain_on_glass.glsl");
const CONFIG_TMPL: &str = include_str!("../assets/ghostty-minimal.conf.tmpl");

/// Write the shader + rendered config under `home` (the entheai config dir,
/// normally `~/.config/entheai`). Idempotent — always (re)writes the bundled
/// copies so a version bump refreshes them. Returns `(config_path, shader_path)`,
/// both absolute.
pub fn materialize_assets(home: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let shaders_dir = home.join("shaders");
    std::fs::create_dir_all(&shaders_dir)?;
    let shader_path = shaders_dir.join("rain_on_glass.glsl");
    std::fs::write(&shader_path, SHADER_SRC)?;

    let shader_abs = shader_path
        .canonicalize()
        .unwrap_or_else(|_| shader_path.clone());
    let config = CONFIG_TMPL.replace("{{SHADER}}", &shader_abs.display().to_string());
    let config_path = home.join("ghostty-minimal.conf");
    std::fs::write(&config_path, config)?;

    Ok((config_path, shader_abs))
}

/// The entheai config dir: `$HOME/.config/entheai`.
pub fn entheai_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("entheai")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_writes_shader_and_renders_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let (config, shader) = materialize_assets(home).unwrap();

        assert!(shader.ends_with("shaders/rain_on_glass.glsl"));
        assert!(shader.is_absolute());
        assert!(std::fs::read_to_string(&shader)
            .unwrap()
            .contains("void mainImage"));

        let conf = std::fs::read_to_string(&config).unwrap();
        assert!(!conf.contains("{{SHADER}}"), "placeholder was rendered");
        assert!(conf.contains(&format!("custom-shader = {}", shader.display())));
        assert!(conf.contains("macos-titlebar-style = hidden"));
    }

    #[test]
    fn materialize_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (c1, s1) = materialize_assets(dir.path()).unwrap();
        let (c2, s2) = materialize_assets(dir.path()).unwrap();
        assert_eq!(c1, c2);
        assert_eq!(s1, s2);
        assert!(std::fs::read_to_string(&s2)
            .unwrap()
            .contains("void mainImage"));
    }
}
