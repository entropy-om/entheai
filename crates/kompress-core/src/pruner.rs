use crate::loss::{asymmetric_loss, effective_score, is_must_keep, DEFAULT_THRESHOLD, LAMBDA};
use crate::types::ContextUnit;
use anyhow::Result;

pub struct Pruner {
    pub threshold: f64,
    pub lambda: f64,
}

impl Pruner {
    pub fn new() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            lambda: LAMBDA,
        }
    }

    pub fn prune(&self, units: Vec<ContextUnit>) -> Result<Vec<ContextUnit>> {
        Ok(units
            .into_iter()
            .filter(|u| {
                // Mechanism B (must-keep override): a hard, score-independent
                // safety net that survives regardless of what the soft
                // score/loss check below decides. Checked first since it can
                // only prevent a prune, never cause one — never worth paying
                // for the score/loss computation once it fires.
                if is_must_keep(&u.content) {
                    return true;
                }
                let score = effective_score(u.score, &u.content);
                let loss = asymmetric_loss(score, self.threshold, self.lambda);
                score > self.threshold || loss < self.lambda * self.threshold
            })
            .collect())
    }
}

impl Default for Pruner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(content: &str, score: f64) -> ContextUnit {
        ContextUnit {
            id: "u".into(),
            content: content.to_string(),
            score,
            layer: [0, 1, 2],
            token_count: content.split_whitespace().count(),
            is_critical_syntactic: false,
        }
    }

    #[test]
    fn must_keep_unit_survives_even_with_score_zero() {
        let pruner = Pruner::new();
        let units = vec![unit("child process exited with code 127", 0.0)];
        let pruned = pruner.prune(units).unwrap();
        assert_eq!(
            pruned.len(),
            1,
            "Mechanism B must force-keep despite score=0.0"
        );
    }

    #[test]
    fn low_score_non_must_keep_unit_is_pruned() {
        let pruner = Pruner::new();
        let units = vec![unit("this is basically just filler prose", 0.0)];
        let pruned = pruner.prune(units).unwrap();
        assert!(pruned.is_empty());
    }
}
