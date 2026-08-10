//! `entheai_skills_add` / `entheai_skills_list`: repo-scoped skill management.
//! Skills install relative to the per-call `cwd` (the first `[skills].dirs`
//! entry, default `skills/`), exactly like `entheai --skills add` from that repo.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::engine::{load_config_for, load_env_for, resolve_cwd};

/// `{url, cwd?} → {added: [{name, slug, path, source, tier, skipped_existing}]}`.
pub async fn entheai_skills_add(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let cwd = resolve_cwd(&args, &server_cwd)?;
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;

    let url = crate::engine::required_str(&args, "url")?;
    let dir_name = cfg
        .skills
        .dirs
        .first()
        .map(String::as_str)
        .unwrap_or("skills");
    let skills_dir = cwd.join(dir_name);

    let added = entheai_skills::remote::add_from_url(&url, &skills_dir).await?;
    let list: Vec<Value> = added
        .iter()
        .map(|a| {
            json!({
                "name": a.name,
                "slug": a.slug,
                "path": a.path.display().to_string(),
                "source": a.source,
                "tier": a.tier,
                "skipped_existing": a.skipped_existing,
            })
        })
        .collect();
    Ok(json!({"added": list, "count": list.len()}))
}

/// `{cwd?} → {skills: [{name, description, path}], count}`.
pub async fn entheai_skills_list(args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    let cwd = resolve_cwd(&args, &server_cwd)?;
    load_env_for(&cwd);
    let cfg = load_config_for(&cwd)?;

    let dirs: Vec<PathBuf> = cfg.skills.dirs.iter().map(|d| cwd.join(d)).collect();
    let reg = entheai_skills::SkillRegistry::discover(&dirs);
    let list: Vec<Value> = reg
        .list()
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "path": s.path.display().to_string(),
            })
        })
        .collect();
    Ok(json!({"skills": list, "count": list.len()}))
}
