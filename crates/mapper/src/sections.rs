/// One markdown-derived section of a prompt: `#`/`##` heading (if any) + body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSection {
    pub heading: Option<String>,
    pub body: String,
}

/// Split `task` into sections on `#`/`##` markdown headings. Lines before the
/// first heading (or the whole text, if no heading is found) become a single
/// section with `heading: None`. List lines (`-`, `*`, `+`, `1.`) are left as
/// part of whichever section's body they fall in — sectioning only reacts to
/// headings.
pub fn split_sections(task: &str) -> Vec<PromptSection> {
    let mut sections = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_body = String::new();

    for line in task.lines() {
        if let Some(heading) = heading_text(line) {
            if !current_body.trim().is_empty() || current_heading.is_some() {
                sections.push(PromptSection {
                    heading: current_heading.take(),
                    body: current_body.trim_end().to_string(),
                });
            }
            current_heading = Some(heading);
            current_body = String::new();
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }

    if !current_body.trim().is_empty() || current_heading.is_some() {
        sections.push(PromptSection {
            heading: current_heading,
            body: current_body.trim_end().to_string(),
        });
    }

    if sections.is_empty() {
        sections.push(PromptSection {
            heading: None,
            body: task.trim_end().to_string(),
        });
    }

    sections
}

/// `# Heading` or `## Heading` -> `Some("Heading")`; anything else -> `None`.
fn heading_text(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    for prefix in ["## ", "# "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_becomes_single_untitled_section() {
        let sections = split_sections("just a plain task, do the thing");
        assert_eq!(
            sections,
            vec![PromptSection {
                heading: None,
                body: "just a plain task, do the thing".to_string(),
            }]
        );
    }

    #[test]
    fn headings_split_into_named_sections() {
        let task = "# Requirements\nDo X\nDo Y\n\n## Constraints\nNo Z\n";
        let sections = split_sections(task);
        assert_eq!(
            sections,
            vec![
                PromptSection {
                    heading: Some("Requirements".to_string()),
                    body: "Do X\nDo Y".to_string(),
                },
                PromptSection {
                    heading: Some("Constraints".to_string()),
                    body: "No Z".to_string(),
                },
            ]
        );
    }

    #[test]
    fn list_lines_stay_inside_their_section_body() {
        let task = "# Steps\n- do X\n- do Y\n1. then Z\n";
        let sections = split_sections(task);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading.as_deref(), Some("Steps"));
        assert_eq!(sections[0].body, "- do X\n- do Y\n1. then Z");
    }

    #[test]
    fn empty_input_yields_one_empty_untitled_section() {
        let sections = split_sections("");
        assert_eq!(
            sections,
            vec![PromptSection {
                heading: None,
                body: String::new(),
            }]
        );
    }
}
