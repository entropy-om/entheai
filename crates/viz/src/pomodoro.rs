//! Pure automatic Pomodoro timer (25 min work / 5 min break), ASCII-only.
//!
//! Terminal-agnostic like `BrainState`: the model is a stateless function of
//! *elapsed seconds since start*, so it never touches the clock and is trivially
//! unit-testable. The TUI computes `elapsed` once per draw and formats the view
//! into the status line. "Automatic" = it cycles forever with no user action.

/// Which half of the cycle we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PomoPhase {
    Work,
    Break,
}

/// Phase + countdown at a given instant, plus how many full cycles have elapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PomodoroView {
    pub phase: PomoPhase,
    pub remaining_secs: u64,
    pub cycle: u64,
}

/// The timer's two durations. Default is the classic 25-on / 5-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pomodoro {
    pub work_secs: u64,
    pub break_secs: u64,
}

impl Default for Pomodoro {
    fn default() -> Self {
        Pomodoro { work_secs: 25 * 60, break_secs: 5 * 60 }
    }
}

impl Pomodoro {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(work_secs: u64, break_secs: u64) -> Self {
        Pomodoro { work_secs, break_secs }
    }

    /// Phase + remaining countdown at `elapsed_secs` since the timer started.
    /// Cycles forever; a zero-length period is guarded (never divides by zero).
    pub fn at(&self, elapsed_secs: u64) -> PomodoroView {
        let period = self.work_secs + self.break_secs;
        if period == 0 {
            return PomodoroView { phase: PomoPhase::Work, remaining_secs: 0, cycle: 0 };
        }
        let cycle = elapsed_secs / period;
        let t = elapsed_secs % period;
        if t < self.work_secs {
            PomodoroView { phase: PomoPhase::Work, remaining_secs: self.work_secs - t, cycle }
        } else {
            PomodoroView { phase: PomoPhase::Break, remaining_secs: period - t, cycle }
        }
    }
}

/// `MM:SS` for a countdown, minutes capped at 99 (pure ASCII).
pub fn fmt_mmss(secs: u64) -> String {
    let m = (secs / 60).min(99);
    let s = secs % 60;
    format!("{m:02}:{s:02}")
}

/// One-line ASCII label, e.g. `WORK 24:59` / `BREAK 04:12`.
pub fn label(view: &PomodoroView) -> String {
    let name = match view.phase {
        PomoPhase::Work => "WORK",
        PomoPhase::Break => "BREAK",
    };
    format!("{name} {}", fmt_mmss(view.remaining_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_work_at_full_duration() {
        let p = Pomodoro::new();
        let v = p.at(0);
        assert_eq!(v.phase, PomoPhase::Work);
        assert_eq!(v.remaining_secs, 25 * 60);
        assert_eq!(v.cycle, 0);
    }

    #[test]
    fn counts_down_within_the_work_block() {
        let p = Pomodoro::new();
        assert_eq!(p.at(1).remaining_secs, 25 * 60 - 1);
        // one second before the break begins
        let v = p.at(25 * 60 - 1);
        assert_eq!(v.phase, PomoPhase::Work);
        assert_eq!(v.remaining_secs, 1);
    }

    #[test]
    fn flips_to_break_at_work_boundary() {
        let p = Pomodoro::new();
        let v = p.at(25 * 60);
        assert_eq!(v.phase, PomoPhase::Break);
        assert_eq!(v.remaining_secs, 5 * 60);
        assert_eq!(v.cycle, 0);
    }

    #[test]
    fn last_break_second_then_wraps_to_next_cycle() {
        let p = Pomodoro::new();
        // final second of the break
        let v = p.at(30 * 60 - 1);
        assert_eq!(v.phase, PomoPhase::Break);
        assert_eq!(v.remaining_secs, 1);
        // the very next second restarts work, cycle advances
        let w = p.at(30 * 60);
        assert_eq!(w.phase, PomoPhase::Work);
        assert_eq!(w.remaining_secs, 25 * 60);
        assert_eq!(w.cycle, 1);
    }

    #[test]
    fn zero_period_does_not_divide_by_zero() {
        let p = Pomodoro::with(0, 0);
        let v = p.at(123);
        assert_eq!(v.remaining_secs, 0);
        assert_eq!(v.cycle, 0);
    }

    #[test]
    fn mmss_formats_and_caps_minutes() {
        assert_eq!(fmt_mmss(25 * 60), "25:00");
        assert_eq!(fmt_mmss(59), "00:59");
        assert_eq!(fmt_mmss(61), "01:01");
        assert_eq!(fmt_mmss(100 * 60), "99:00", "minutes cap at 99");
    }

    #[test]
    fn label_is_ascii_and_names_the_phase() {
        let work = label(&PomodoroView { phase: PomoPhase::Work, remaining_secs: 25 * 60 - 1, cycle: 0 });
        assert_eq!(work, "WORK 24:59");
        let brk = label(&PomodoroView { phase: PomoPhase::Break, remaining_secs: 4 * 60 + 12, cycle: 3 });
        assert_eq!(brk, "BREAK 04:12");
        assert!(work.is_ascii() && brk.is_ascii(), "pure ASCII");
    }
}
