//! VRR-Stop — robust stopping for noisy verify→repair loops.
//!
//! ## Honest scope (read this first)
//!
//! entheai verifies each fan-out leaf **once** today: see `run_fanout` in
//! [`crate`], where `verify_worktree` is called a single time and a `Failed`
//! result is composted to the trajectory sink rather than repaired. **There is
//! no repair loop yet.** This module does not pretend otherwise. It is the
//! *governor* a repair loop must consult — implemented and tested now so that
//! when the repair *actor* lands (re-dispatching a leaf to fix a failed diff),
//! the loop is safe by construction instead of bolted on. The remaining piece
//! is the actor, not this decision.
//!
//! ## What it implements
//!
//! Wu et al. 2026, *Verify, Repair, Repeat, or Stop?* (arXiv:2607.17641). The
//! core observation: when the verifier is itself fallible, repeated repair can
//! **destroy a correct state** even while the verifier's reported acceptance
//! rises. Two consequences, both encoded here:
//!
//! 1. **Keep the best-verified incumbent, never "the latest" by default.**
//!    A repair is an irreversible intervention on the trajectory; detecting a
//!    failure does not make repairing it admissible.
//! 2. **Only replace the incumbent when a candidate clears a verification
//!    margin**, and only keep repairing while another attempt has plausible
//!    positive marginal value. When the verifier can't discriminate (gives no
//!    gradient), fall back to preserving the incumbent.

/// One verify outcome for a candidate the loop produced.
#[derive(Clone, Debug)]
pub struct Attempt {
    /// Did the verify gate pass outright? A pass dominates any non-pass.
    pub passed: bool,
    /// A monotone "closer to green" signal when the verifier can produce one
    /// (e.g. negated failing-test count). Higher is better. `None` means the
    /// verifier gave no gradient — the weak-verifier case.
    pub score: Option<f64>,
    /// Opaque handle to the candidate state to keep if chosen (a branch, a seal).
    pub candidate: String,
}

/// Why the loop stopped, for honest reporting up the stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// A candidate passed the gate outright.
    Passed,
    /// The attempt budget was exhausted.
    MaxAttempts,
    /// No recent attempt cleared the improvement margin — another repair has no
    /// evidenced marginal value, so stop and keep the incumbent.
    NoMarginalValue,
    /// The verifier produced no gradient and nothing passed — it can't tell us
    /// whether a repair helps, so preserve the incumbent rather than churn it.
    WeakVerifier,
}

/// The decision after recording an attempt.
#[derive(Clone, Debug)]
pub struct RepairDecision {
    /// Should the loop attempt another repair?
    pub keep_repairing: bool,
    /// The candidate to KEEP right now — the best-verified so far, which is
    /// *not* necessarily the most recent attempt. This is the whole point.
    pub incumbent: Option<String>,
    /// Set when `keep_repairing` is false.
    pub stop_reason: Option<StopReason>,
}

/// Tracks a verify→repair loop and rules on when to stop, per VRR-Stop.
#[derive(Clone, Debug)]
pub struct RepairLedger {
    max_attempts: u32,
    /// A candidate must beat the incumbent's score by at least this much to
    /// replace it — noise below the margin must not churn the incumbent.
    margin: f64,
    attempts: Vec<Attempt>,
    /// Index into `attempts` of the current incumbent.
    incumbent: Option<usize>,
}

impl RepairLedger {
    /// `max_attempts` caps repair rounds; `margin` is the verification margin a
    /// candidate must clear to replace the incumbent (use `0.0` to accept any
    /// strict improvement). A `max_attempts` of 0 means "no repair at all" —
    /// the loop records the first result and stops, matching entheai's current
    /// verify-once behaviour exactly.
    pub fn new(max_attempts: u32, margin: f64) -> Self {
        Self {
            max_attempts,
            margin: margin.max(0.0),
            attempts: Vec::new(),
            incumbent: None,
        }
    }

    /// The candidate currently held as best-verified, if any.
    pub fn incumbent(&self) -> Option<&str> {
        self.incumbent.map(|i| self.attempts[i].candidate.as_str())
    }

    /// Record a fresh attempt and rule on the loop. Faithful to VRR-Stop:
    ///
    /// - A **pass** always becomes the incumbent and stops the loop.
    /// - Otherwise the incumbent is only *replaced* by a candidate whose score
    ///   beats it by `>= margin`; below that, the incumbent stands (repair does
    ///   not get to overwrite a better prior state on noise).
    /// - The loop **keeps repairing** only while attempts remain *and* the best
    ///   score is still improving by the margin. Stagnation → `NoMarginalValue`.
    /// - If no attempt ever passed and none carried a score, the verifier is
    ///   too weak to justify continued repair → `WeakVerifier`, incumbent kept.
    pub fn record(&mut self, attempt: Attempt) -> RepairDecision {
        let passed = attempt.passed;
        let idx = self.attempts.len();
        self.attempts.push(attempt);
        self.update_incumbent(idx);

        // A verified pass dominates everything: keep it, stop.
        if passed {
            return self.stop(StopReason::Passed);
        }
        // Budget exhausted (attempts made == max repair rounds + the initial one).
        if self.attempts.len() as u32 > self.max_attempts {
            return self.stop(StopReason::MaxAttempts);
        }
        // Verifier gave no gradient at all and nothing passed: don't churn.
        if self.attempts.iter().all(|a| a.score.is_none()) {
            return self.stop(StopReason::WeakVerifier);
        }
        // Marginal value: has the best score improved by >= margin recently? If
        // the newest attempt failed to advance the incumbent, another repair has
        // no evidenced value.
        if self.incumbent != Some(idx) {
            return self.stop(StopReason::NoMarginalValue);
        }
        RepairDecision {
            keep_repairing: true,
            incumbent: self.incumbent_owned(),
            stop_reason: None,
        }
    }

    /// A candidate replaces the incumbent iff there is no incumbent yet, or it
    /// passed, or its score beats the incumbent's by at least `margin`.
    fn update_incumbent(&mut self, idx: usize) {
        let cand = &self.attempts[idx];
        let replace = match self.incumbent {
            None => true,
            Some(cur) => {
                let inc = &self.attempts[cur];
                if cand.passed && !inc.passed {
                    true
                } else if inc.passed {
                    false // a verified pass is never displaced by a non-pass
                } else {
                    match (cand.score, inc.score) {
                        (Some(c), Some(i)) => c >= i + self.margin,
                        _ => false, // no gradient → keep the earlier incumbent
                    }
                }
            }
        };
        if replace {
            self.incumbent = Some(idx);
        }
    }

    fn incumbent_owned(&self) -> Option<String> {
        self.incumbent.map(|i| self.attempts[i].candidate.clone())
    }

    fn stop(&self, reason: StopReason) -> RepairDecision {
        RepairDecision {
            keep_repairing: false,
            incumbent: self.incumbent_owned(),
            stop_reason: Some(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(passed: bool, score: Option<f64>, c: &str) -> Attempt {
        Attempt {
            passed,
            score,
            candidate: c.to_string(),
        }
    }

    #[test]
    fn a_pass_is_kept_and_stops() {
        let mut l = RepairLedger::new(5, 0.0);
        let d = l.record(att(true, Some(0.0), "green"));
        assert!(!d.keep_repairing);
        assert_eq!(d.stop_reason, Some(StopReason::Passed));
        assert_eq!(d.incumbent.as_deref(), Some("green"));
    }

    #[test]
    fn max_attempts_of_zero_is_verify_once() {
        // Matches entheai's current behaviour: record once, stop, no repair.
        let mut l = RepairLedger::new(0, 0.0);
        let d = l.record(att(false, Some(-3.0), "first"));
        assert!(!d.keep_repairing);
        assert_eq!(d.stop_reason, Some(StopReason::MaxAttempts));
        assert_eq!(d.incumbent.as_deref(), Some("first"));
    }

    #[test]
    fn repeated_no_improvement_keeps_the_incumbent_not_the_latest() {
        // The core VRR-Stop property: noisy repair must not destroy a better
        // prior state. First attempt is the best; later ones don't clear the
        // margin, so the incumbent stays "best", and we stop for no value.
        let mut l = RepairLedger::new(5, 1.0);
        let d0 = l.record(att(false, Some(-2.0), "best")); // 2 failing tests
        assert!(d0.keep_repairing);
        let d1 = l.record(att(false, Some(-2.0), "worse-a")); // no improvement
        assert!(!d1.keep_repairing);
        assert_eq!(d1.stop_reason, Some(StopReason::NoMarginalValue));
        assert_eq!(d1.incumbent.as_deref(), Some("best")); // NOT "worse-a"
    }

    #[test]
    fn a_candidate_clearing_the_margin_becomes_incumbent_and_loop_continues() {
        let mut l = RepairLedger::new(5, 1.0);
        assert!(l.record(att(false, Some(-4.0), "c0")).keep_repairing);
        let d = l.record(att(false, Some(-2.0), "c1")); // improves by 2 >= margin 1
        assert!(d.keep_repairing);
        assert_eq!(d.incumbent.as_deref(), Some("c1"));
    }

    #[test]
    fn improvement_below_margin_does_not_churn_the_incumbent() {
        let mut l = RepairLedger::new(5, 1.0);
        l.record(att(false, Some(-4.0), "c0"));
        let d = l.record(att(false, Some(-3.5), "c1")); // +0.5 < margin 1.0
        assert!(!d.keep_repairing);
        assert_eq!(d.stop_reason, Some(StopReason::NoMarginalValue));
        assert_eq!(d.incumbent.as_deref(), Some("c0"));
    }

    #[test]
    fn a_weak_verifier_preserves_the_incumbent() {
        let mut l = RepairLedger::new(5, 0.0);
        let d = l.record(att(false, None, "only-candidate"));
        assert!(!d.keep_repairing);
        assert_eq!(d.stop_reason, Some(StopReason::WeakVerifier));
        assert_eq!(d.incumbent.as_deref(), Some("only-candidate"));
    }

    #[test]
    fn a_later_pass_displaces_a_scored_incumbent() {
        let mut l = RepairLedger::new(5, 1.0);
        l.record(att(false, Some(-1.0), "fail")); // incumbent = fail
        let d = l.record(att(true, None, "pass")); // pass dominates
        assert!(!d.keep_repairing);
        assert_eq!(d.stop_reason, Some(StopReason::Passed));
        assert_eq!(d.incumbent.as_deref(), Some("pass"));
    }
}
