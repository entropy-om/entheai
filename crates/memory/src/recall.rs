//! Pure recall scoring: reciprocal-rank fusion of ranked id lists, recency
//! decay, and the final blended score. No DB or async — trivially testable.

use std::collections::HashMap;

/// Weights + constants for the final blended score (from config; see Task 5).
#[derive(Debug, Clone, Copy)]
pub struct ScoreWeights {
    pub w_recency: f64,
    pub w_conf: f64,
    pub half_life_days: f64,
    pub rrf_k: f64,
}

/// Reciprocal-rank fusion over several ranked id lists (each best-first):
/// `rrf(id) = Σ_i 1 / (k + rank_i)`, rank 1-based. Ids absent from a list
/// contribute nothing from it.
pub fn rrf(lists: &[&[i64]], k: f64) -> HashMap<i64, f64> {
    // Guard against a config-supplied `k` that would make the denominator ≤ 0.
    // rank is 1-based, so with `k ≥ 0` we always have `k + rank ≥ 1`.
    let k = k.max(0.0);
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in lists {
        for (rank0, &id) in list.iter().enumerate() {
            *scores.entry(id).or_insert(0.0) += 1.0 / (k + (rank0 as f64 + 1.0));
        }
    }
    scores
}

/// Min-max normalize a score map into [0,1] so it's comparable to the recency
/// and confidence terms before blending. When all scores are equal (or there's
/// one entry), every score maps to 1.0 (all equally relevant).
pub fn normalize(scores: &HashMap<i64, f64>) -> HashMap<i64, f64> {
    let min = scores.values().copied().fold(f64::INFINITY, f64::min);
    let max = scores.values().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    scores
        .iter()
        .map(|(&id, &s)| {
            let n = if range > f64::EPSILON {
                (s - min) / range
            } else {
                1.0
            };
            (id, n)
        })
        .collect()
}

/// Exponential recency term: `exp(-age_days / half_life_days)`.
/// `decay(now, now) == 1.0`; strictly decreasing in age.
pub fn recency_decay(created_at_ms: i64, now_ms: i64, half_life_days: f64) -> f64 {
    let age_days = ((now_ms - created_at_ms).max(0) as f64) / 86_400_000.0;
    (-age_days / half_life_days.max(f64::EPSILON)).exp()
}

/// `final = rrf + w_recency·recency + w_conf·confidence`.
pub fn final_score(rrf_score: f64, recency: f64, confidence: f64, w: &ScoreWeights) -> f64 {
    rrf_score + w.w_recency * recency + w.w_conf * confidence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_rewards_appearing_high_in_both_lists() {
        // id 7 is rank 1 in list A and rank 1 in list B → highest.
        let a = [7_i64, 3, 9];
        let b = [7_i64, 1, 2];
        let scores = rrf(&[&a, &b], 60.0);
        let top = scores
            .iter()
            .max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
            .unwrap();
        assert_eq!(*top.0, 7);
    }

    #[test]
    fn rrf_single_list_ranks_by_position() {
        let a = [10_i64, 20, 30];
        let s = rrf(&[&a], 60.0);
        assert!(s[&10] > s[&20] && s[&20] > s[&30]);
    }

    #[test]
    fn decay_is_one_at_zero_age_and_monotone() {
        let now = 1_000_000_000_000;
        assert!((recency_decay(now, now, 14.0) - 1.0).abs() < 1e-9);
        let day = 86_400_000_i64;
        let d1 = recency_decay(now - day, now, 14.0);
        let d10 = recency_decay(now - 10 * day, now, 14.0);
        assert!(d1 < 1.0 && d10 < d1);
    }

    #[test]
    fn final_score_blends_terms() {
        let w = ScoreWeights {
            w_recency: 0.3,
            w_conf: 0.2,
            half_life_days: 14.0,
            rrf_k: 60.0,
        };
        let s = final_score(0.5, 1.0, 0.5, &w);
        assert!((s - (0.5 + 0.3 * 1.0 + 0.2 * 0.5)).abs() < 1e-9);
    }

    #[test]
    fn normalize_maps_to_unit_range() {
        let m = HashMap::from([(1_i64, 0.033), (2, 0.016), (3, 0.022)]);
        let n = normalize(&m);
        assert!((n[&1] - 1.0).abs() < 1e-9);
        assert!((n[&2] - 0.0).abs() < 1e-9);
        assert!(n[&3] > 0.0 && n[&3] < 1.0);
    }

    #[test]
    fn normalize_single_or_all_equal_is_one() {
        assert!((normalize(&HashMap::from([(1_i64, 0.5)]))[&1] - 1.0).abs() < 1e-9);
        let n = normalize(&HashMap::from([(1_i64, 0.2), (2, 0.2)]));
        assert!((n[&1] - 1.0).abs() < 1e-9 && (n[&2] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn relevance_outranks_moderate_recency_gap_after_normalize() {
        // Post-normalize: strong relevance (1.0) but older (recency 0.8) still beats
        // weak relevance (0.0) that's fresh (recency 1.0) — relevance drives ranking.
        let w = ScoreWeights {
            w_recency: 0.3,
            w_conf: 0.2,
            half_life_days: 14.0,
            rrf_k: 60.0,
        };
        let strong_old = final_score(1.0, 0.8, 0.5, &w);
        let weak_fresh = final_score(0.0, 1.0, 0.5, &w);
        assert!(strong_old > weak_fresh, "{strong_old} !> {weak_fresh}");
    }
}
