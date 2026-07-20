//! Skills: discover Claude Agent-Skills-format `SKILL.md` files (YAML frontmatter
//! with `name` + `description`, then a markdown body of instructions), advertise
//! them to the agent, and load one's full instructions on demand via the `skill` tool.

pub mod remote;

use std::path::PathBuf;
use std::sync::Arc;

/// One discovered skill.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String, // the instructions (everything after frontmatter)
    pub path: PathBuf,
}

/// Parse a SKILL.md's `---`-fenced frontmatter for `name`/`description` and return
/// (name, description, body). Falls back to the dir name for `name` if absent.
/// Frontmatter is minimal `key: value` lines (strip surrounding quotes); the body
/// is everything after the closing `---` (or the whole text if no frontmatter).
pub fn parse_skill_md(text: &str, fallback_name: &str) -> (String, String, String) {
    if let Some(rest) = text.strip_prefix("---\n") {
        // Find the closing fence: a line that is exactly "---".
        let mut end_idx = None;
        let mut search_from = 0usize;
        for line in rest.split('\n') {
            if line == "---" {
                end_idx = Some(search_from);
                break;
            }
            search_from += line.len() + 1; // +1 for the '\n' separator
        }

        if let Some(idx) = end_idx {
            let frontmatter = &rest[..idx];
            // Body starts right after the closing "---" line.
            let after_fence = &rest[idx + "---".len()..];
            let body = after_fence.strip_prefix('\n').unwrap_or(after_fence);

            let mut name = fallback_name.to_string();
            let mut description = String::new();
            for line in frontmatter.lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = strip_quotes(v.trim());
                } else if let Some(v) = line.strip_prefix("description:") {
                    description = strip_quotes(v.trim());
                }
            }

            return (name, description, body.to_string());
        }
    }

    // No frontmatter (or no closing fence found): whole text is the body.
    (fallback_name.to_string(), String::new(), text.to_string())
}

/// Strip a single layer of matching surrounding `"` or `'` quotes, if present.
fn strip_quotes(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: Vec<Skill>,
}

impl SkillRegistry {
    /// Scan each dir for immediate sub-directories containing a `SKILL.md`.
    /// Missing dirs are skipped silently. Skills are sorted by name; on duplicate
    /// names the first discovered wins.
    pub fn discover(dirs: &[PathBuf]) -> Self {
        let mut skills: Vec<Skill> = Vec::new();

        for dir in dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(_) => continue, // missing dir: skip silently
            };
            for entry in entries.flatten() {
                let sub_path = entry.path();
                if !sub_path.is_dir() {
                    continue;
                }
                let skill_md = sub_path.join("SKILL.md");
                let text = match std::fs::read_to_string(&skill_md) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let fallback_name = sub_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let (name, description, body) = parse_skill_md(&text, &fallback_name);

                if skills.iter().any(|s: &Skill| s.name == name) {
                    continue; // first discovered wins
                }
                skills.push(Skill {
                    name,
                    description,
                    body,
                    path: skill_md,
                });
            }
        }

        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Self { skills }
    }

    pub fn list(&self) -> &[Skill] {
        &self.skills
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// System-prompt snippet advertising the skills (empty string if none).
    pub fn advertisement(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut s = String::from(
            "You have skills available. To use one, call the `skill` tool with its `name` to load its full instructions, then follow them.\n\nAvailable skills:\n",
        );
        for sk in &self.skills {
            s.push_str(&format!("- {}: {}\n", sk.name, sk.description));
        }
        s
    }
}

/// The `skill` tool: `skill({"name": "<skill>"})` returns that skill's instructions.
pub struct SkillTool {
    registry: Arc<SkillRegistry>,
}

impl SkillTool {
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl entheai_tools::Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type":"function",
            "function":{
                "name":"skill",
                "description":"Load a skill's full instructions by name. Call this when a listed skill fits the task, then follow the returned instructions.",
                "parameters":{"type":"object","properties":{"name":{"type":"string","description":"The skill name"}},"required":["name"]}
            }
        })
    }
    async fn call(&self, args: serde_json::Value) -> Result<String, entheai_tools::ToolError> {
        let name = args["name"]
            .as_str()
            .ok_or_else(|| entheai_tools::ToolError::MissingArg("name".into()))?;
        match self.registry.get(name) {
            Some(sk) => Ok(sk.body.clone()),
            None => {
                let avail: Vec<&str> = self
                    .registry
                    .list()
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect();
                Ok(format!(
                    "error: no skill named '{name}'. Available: {}",
                    avail.join(", ")
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entheai_tools::Tool;
    use std::fs;
    use std::path::Path;

    fn write_skill(dir: &Path, name_dir: &str, contents: &str) {
        let sk_dir = dir.join(name_dir);
        fs::create_dir_all(&sk_dir).unwrap();
        fs::write(sk_dir.join("SKILL.md"), contents).unwrap();
    }

    #[test]
    fn parse_skill_md_with_frontmatter() {
        let text =
            "---\nname: my-skill\ndescription: does a thing\n---\n# Body\n\nInstructions here.\n";
        let (name, description, body) = parse_skill_md(text, "fallback");
        assert_eq!(name, "my-skill");
        assert_eq!(description, "does a thing");
        assert_eq!(body, "# Body\n\nInstructions here.\n");
    }

    #[test]
    fn parse_skill_md_with_quoted_values() {
        let text = "---\nname: \"quoted-name\"\ndescription: 'single quoted'\n---\nBody text\n";
        let (name, description, body) = parse_skill_md(text, "fallback");
        assert_eq!(name, "quoted-name");
        assert_eq!(description, "single quoted");
        assert_eq!(body, "Body text\n");
    }

    #[test]
    fn parse_skill_md_without_frontmatter() {
        let text = "Just a plain markdown file, no frontmatter.\n";
        let (name, description, body) = parse_skill_md(text, "fallback-name");
        assert_eq!(name, "fallback-name");
        assert_eq!(description, "");
        assert_eq!(body, text);
    }

    #[test]
    fn discover_finds_skills_sorted_and_skips_missing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "foo",
            "---\nname: foo\ndescription: foo skill\n---\nfoo body\n",
        );
        write_skill(
            dir.path(),
            "bar",
            "---\nname: bar\ndescription: bar skill\n---\nbar body\n",
        );

        let missing = dir.path().join("does-not-exist");
        let registry = SkillRegistry::discover(&[dir.path().to_path_buf(), missing]);

        assert_eq!(registry.list().len(), 2);
        assert_eq!(registry.list()[0].name, "bar");
        assert_eq!(registry.list()[1].name, "foo");
    }

    #[test]
    fn advertisement_empty_when_no_skills() {
        let registry = SkillRegistry::default();
        assert_eq!(registry.advertisement(), "");
    }

    #[test]
    fn advertisement_lists_names_and_descriptions() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "foo",
            "---\nname: foo\ndescription: foo skill\n---\nfoo body\n",
        );
        let registry = SkillRegistry::discover(&[dir.path().to_path_buf()]);
        let ad = registry.advertisement();
        assert!(ad.contains("call the `skill` tool"));
        assert!(ad.contains("foo"));
        assert!(ad.contains("foo skill"));
    }

    #[tokio::test]
    async fn skill_tool_returns_body_for_known_name() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "foo",
            "---\nname: foo\ndescription: foo skill\n---\nfoo body\n",
        );
        let registry = Arc::new(SkillRegistry::discover(&[dir.path().to_path_buf()]));
        let tool = SkillTool::new(registry);
        let out = tool.call(serde_json::json!({"name": "foo"})).await.unwrap();
        assert_eq!(out, "foo body\n");
    }

    #[tokio::test]
    async fn skill_tool_errors_for_unknown_name() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "foo",
            "---\nname: foo\ndescription: foo skill\n---\nfoo body\n",
        );
        let registry = Arc::new(SkillRegistry::discover(&[dir.path().to_path_buf()]));
        let tool = SkillTool::new(registry);
        let out = tool
            .call(serde_json::json!({"name": "nope"}))
            .await
            .unwrap();
        assert!(out.contains("no skill named"));
        assert!(out.contains("foo"));
    }
}
