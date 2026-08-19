use crate::types::ContextUnit;
use anyhow::Result;

const MAX_TOKENS: usize = 60;

const FILLER: &[&str] = &[
    "basically",
    "essentially",
    "actually",
    "just",
    "very",
    "really",
    "quite",
    "simply",
    "obviously",
    "clearly",
    "of course",
    "you know",
];

pub struct Rewriter {
    pub max_tokens: usize,
}

impl Rewriter {
    pub fn new() -> Self {
        Self {
            max_tokens: MAX_TOKENS,
        }
    }

    pub fn rewrite(&self, units: Vec<ContextUnit>) -> Result<Vec<ContextUnit>> {
        Ok(units
            .into_iter()
            .map(|mut u| {
                let mut text = u.content.clone();
                for filler in FILLER {
                    text = text.replace(&format!(" {filler} "), " ");
                }
                let words: Vec<&str> = text.split_whitespace().collect();
                // Never truncate a must-keep unit (paths, hashes, exit codes,
                // identifiers past word 60) — that defeats the pruner's
                // Mechanism B force-keep guarantee.
                if words.len() > self.max_tokens
                    && !u.is_critical_syntactic
                    && !crate::loss::is_must_keep(&text)
                {
                    text = words[..self.max_tokens].join(" ") + "…";
                }
                u.token_count = text.split_whitespace().count();
                u.content = text;
                u
            })
            .collect())
    }
}

impl Default for Rewriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(content: &str, is_critical_syntactic: bool) -> ContextUnit {
        ContextUnit {
            id: "u".into(),
            content: content.to_string(),
            score: 1.0,
            layer: [0, 1, 2],
            token_count: content.split_whitespace().count(),
            is_critical_syntactic,
        }
    }

    #[test]
    fn a_must_keep_unit_past_max_tokens_is_never_truncated() {
        let long_path = format!(
            "{} /home/user/project/src/very/deeply/nested/module/main.rs",
            "word ".repeat(70)
        );
        let rewriter = Rewriter::new();
        let out = rewriter.rewrite(vec![unit(&long_path, false)]).unwrap();
        assert!(
            out[0].content.contains("main.rs"),
            "must-keep path was truncated away: {}",
            out[0].content
        );
        assert!(!out[0].content.ends_with('…'));
    }

    #[test]
    fn a_critical_syntactic_unit_past_max_tokens_is_never_truncated() {
        let long_text = format!("{}/usr/bin/cargo", "word ".repeat(70));
        let rewriter = Rewriter::new();
        let out = rewriter.rewrite(vec![unit(&long_text, true)]).unwrap();
        assert!(
            out[0].content.contains("/usr/bin/cargo"),
            "critical-syntactic unit was truncated away: {}",
            out[0].content
        );
        assert!(!out[0].content.ends_with('…'));
    }

    #[test]
    fn an_ordinary_unit_past_max_tokens_is_still_truncated() {
        let long_text = "word ".repeat(70);
        let rewriter = Rewriter::new();
        let out = rewriter.rewrite(vec![unit(&long_text, false)]).unwrap();
        assert!(out[0].content.ends_with('…'));
    }
}
