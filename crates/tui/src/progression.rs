//! Player-style progression for the interactive TUI: xp earned from prompts,
//! fan-outs, and focus time, mapped onto a level ladder and unlockable badges.
//! Persisted to `~/.config/entheai/progression.json` — loading never fails and
//! saving is best-effort (award → dirty flag → ticker flushes via `tokio::spawn`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// On-disk schema version. Bump + add migration in [`load`] when the shape
/// of [`Progression`] changes.
pub const PROGRESSION_VERSION: u64 = 1;

/// Badges unlocked by meeting counter thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum BadgeId {
    /// Submitted a prompt.
    FirstPrompt,
    /// Completed a fan-out run.
    FirstFanout,
    /// Merged the first coder (one seal per merged coder).
    FirstSeal,
    /// Fanned out to ≥ 5 workers at once.
    Quintet,
    /// Saw a killed/timed-out worker.
    GhostWhisperer,
    /// Spent ≥ 300s in the Zen field.
    ZenBath,
    /// Amassed ≥ 10 seals.
    SealHoarder,
    /// Submitted ≥ 100 prompts.
    Century,
}

/// The level ladder: (xp threshold, level title). Level `n` begins at
/// `LEVELS[n - 1].0`; `LEVELS[level - 1].1` is its title.
pub const LEVELS: &[(u32, &str)] = &[
    (0, "Spark"),
    (100, "Quant"),
    (250, "Ternary"),
    (450, "Weaver"),
    (750, "Orchestrator"),
    (1150, "Seal-Shaper"),
    (1650, "Field-Archon"),
    (2300, "Entheist"),
];

/// (badge, glyph, name) — the glyph is the badge's emoji on the dashboard.
pub const BADGES: &[(BadgeId, char, &str)] = &[
    (BadgeId::FirstPrompt, '🜂', "First Flame"),
    (BadgeId::FirstFanout, '🕊', "First Flight"),
    (BadgeId::FirstSeal, '🧿', "First Seal"),
    (BadgeId::Quintet, '⧉', "Quintet"),
    (BadgeId::GhostWhisperer, '⚠', "Ghost Whisperer"),
    (BadgeId::ZenBath, '🌊', "Zen Bath"),
    (BadgeId::SealHoarder, '🏛', "Seal Hoarder"),
    (BadgeId::Century, '⚡', "Century"),
];

/// One thing the user did that earns xp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressionEvent {
    /// A prompt was submitted to the agent (not a slash command).
    PromptSubmitted,
    /// A fan-out decomposed into `n` sub-tasks.
    FanoutDecomposed(u32),
    /// A coder sub-agent finished.
    CoderFinished,
    /// The fan-out entered its integrate phase.
    Integrating,
    /// The fan-out finished, merging `merged` branches.
    FanoutDone { merged: usize },
    /// A worker was killed via `/workers stop`.
    WorkerKilled,
    /// One full minute of cumulative Zen-field time.
    ZenTick,
    /// A Pomodoro work block completed (Work → Break flip).
    PomodoroCompleted,
}

/// XP awarded for an event. `FanoutDecomposed` caps at 40; a `FanoutDone`
/// that merged nothing is worth a fraction of one that merged branches.
pub fn xp_for(ev: &ProgressionEvent) -> u64 {
    match ev {
        ProgressionEvent::PromptSubmitted => 10,
        ProgressionEvent::FanoutDecomposed(n) => (5 * n).min(40) as u64,
        ProgressionEvent::CoderFinished => 20,
        ProgressionEvent::Integrating => 10,
        ProgressionEvent::FanoutDone { merged } => {
            if *merged >= 1 {
                40
            } else {
                10
            }
        }
        ProgressionEvent::WorkerKilled => 5,
        ProgressionEvent::ZenTick => 2,
        ProgressionEvent::PomodoroCompleted => 15,
    }
}

/// What a single [`Progression::award`] call changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Award {
    /// XP gained by this event.
    pub xp: u64,
    /// `Some((level, title))` when this event crossed a level threshold.
    pub level_up: Option<(u32, &'static str)>,
    /// Badges newly unlocked by this event, in `BADGES` order.
    pub unlocked: Vec<BadgeId>,
}

/// Persisted player progression.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progression {
    /// On-disk schema version ([`PROGRESSION_VERSION`]).
    pub version: u64,
    /// Total lifetime xp.
    pub xp: u64,
    /// Current level (1-based, recomputed on award).
    pub level: u32,
    /// Unlocked badges, in unlock order.
    pub badges: Vec<BadgeId>,
    /// Prompts submitted.
    pub prompts: u64,
    /// Completed fan-out runs.
    pub fanouts_done: u64,
    /// Coders merged across all fan-outs (one seal per merged coder).
    pub seals: u64,
    /// Largest single fan-out decomposition seen.
    pub max_workers: u64,
    /// Workers killed via `/workers stop`.
    pub timed_out_seen: u64,
    /// Cumulative seconds spent in the Zen field.
    pub zen_seconds: u64,
    /// Completed Pomodoro work blocks.
    pub pomodoros: u64,
}

impl Progression {
    /// The 1-based level for `xp`: the count of `LEVELS` thresholds ≤ `xp`.
    pub fn level_of(xp: u64) -> u32 {
        LEVELS
            .iter()
            .filter(|(threshold, _)| *threshold as u64 <= xp)
            .count() as u32
    }

    /// The title of `level`, clamped to the top of the ladder.
    pub fn level_title(level: u32) -> &'static str {
        let idx = (level.saturating_sub(1) as usize).min(LEVELS.len() - 1);
        LEVELS[idx].1
    }

    /// The xp threshold where `level` begins.
    pub fn level_threshold(level: u32) -> u64 {
        let idx = (level.saturating_sub(1) as usize).min(LEVELS.len() - 1);
        LEVELS[idx].0 as u64
    }

    /// The xp threshold where the next level begins (`None` at max level).
    pub fn next_level_threshold(level: u32) -> Option<u64> {
        LEVELS.get(level as usize).map(|(t, _)| *t as u64)
    }

    /// `"{glyph} {name}"` of the next badge not yet unlocked, or `"max"` once
    /// every badge is earned.
    pub fn next_badge_label(&self) -> String {
        for (id, glyph, name) in BADGES {
            if !self.badges.contains(id) {
                return format!("{glyph} {name}");
            }
        }
        "max".to_string()
    }

    /// Badges whose conditions the current counters satisfy, in `BADGES` order.
    pub fn badges_for(&self) -> Vec<BadgeId> {
        let mut out = Vec::new();
        if self.prompts >= 1 {
            out.push(BadgeId::FirstPrompt);
        }
        if self.fanouts_done >= 1 {
            out.push(BadgeId::FirstFanout);
        }
        if self.seals >= 1 {
            out.push(BadgeId::FirstSeal);
        }
        if self.max_workers >= 5 {
            out.push(BadgeId::Quintet);
        }
        if self.timed_out_seen >= 1 {
            out.push(BadgeId::GhostWhisperer);
        }
        if self.zen_seconds >= 300 {
            out.push(BadgeId::ZenBath);
        }
        if self.seals >= 10 {
            out.push(BadgeId::SealHoarder);
        }
        if self.prompts >= 100 {
            out.push(BadgeId::Century);
        }
        out
    }

    /// Apply `ev`: bump counters, add xp, recompute the level (count of
    /// `LEVELS` thresholds ≤ xp), and unlock any newly-earned badges.
    pub fn award(&mut self, ev: ProgressionEvent) -> Award {
        let xp = xp_for(&ev);
        self.xp += xp;
        match ev {
            ProgressionEvent::PromptSubmitted => self.prompts += 1,
            ProgressionEvent::FanoutDecomposed(n) => {
                self.max_workers = self.max_workers.max(n as u64);
            }
            ProgressionEvent::CoderFinished => {}
            ProgressionEvent::Integrating => {}
            ProgressionEvent::FanoutDone { merged } => {
                self.fanouts_done += 1;
                self.seals += merged as u64;
            }
            ProgressionEvent::WorkerKilled => self.timed_out_seen += 1,
            ProgressionEvent::ZenTick => self.zen_seconds += 60,
            ProgressionEvent::PomodoroCompleted => self.pomodoros += 1,
        }
        let new_level = Self::level_of(self.xp);
        let level_up = if new_level > self.level {
            self.level = new_level;
            Some((new_level, Self::level_title(new_level)))
        } else {
            None
        };
        let unlocked: Vec<BadgeId> = self
            .badges_for()
            .into_iter()
            .filter(|b| !self.badges.contains(b))
            .collect();
        self.badges.extend(unlocked.iter().copied());
        Award {
            xp,
            level_up,
            unlocked,
        }
    }

    /// Persist to disk via a temp file + rename so a crash can't corrupt the
    /// existing file. Best-effort: callers ignore the result.
    pub async fn save(&self) -> anyhow::Result<()> {
        let path = progression_path();
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let data = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, data).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }
}

/// The display name of a badge (from [`BADGES`]).
pub fn badge_name(id: BadgeId) -> &'static str {
    BADGES
        .iter()
        .find(|(b, _, _)| *b == id)
        .map(|(_, _, name)| *name)
        .unwrap_or("")
}

/// `~/.config/entheai/progression.json` (HOME-based — no extra crates).
pub fn progression_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config")
        .join("entheai")
        .join("progression.json")
}

/// Load the saved progression; `Default` on any error so startup never fails.
pub fn load() -> Progression {
    let path = progression_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => Progression::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_ladder_thresholds() {
        assert_eq!(Progression::level_of(0), 1);
        assert_eq!(Progression::level_of(99), 1);
        assert_eq!(Progression::level_of(100), 2);
        assert_eq!(Progression::level_of(249), 2);
        assert_eq!(Progression::level_of(2300), 8);
        assert_eq!(Progression::level_of(5000), 8);
        assert_eq!(Progression::level_title(1), "Spark");
        assert_eq!(Progression::level_title(2), "Quant");
        assert_eq!(Progression::level_title(8), "Entheist");
        assert_eq!(Progression::level_threshold(3), 250);
        assert_eq!(Progression::next_level_threshold(3), Some(450));
        assert_eq!(Progression::next_level_threshold(8), None);
    }

    #[test]
    fn xp_for_maps_all_events() {
        assert_eq!(xp_for(&ProgressionEvent::PromptSubmitted), 10);
        assert_eq!(xp_for(&ProgressionEvent::FanoutDecomposed(3)), 15);
        assert_eq!(xp_for(&ProgressionEvent::FanoutDecomposed(8)), 40);
        assert_eq!(
            xp_for(&ProgressionEvent::FanoutDecomposed(12)),
            40,
            "capped at 40"
        );
        assert_eq!(xp_for(&ProgressionEvent::FanoutDecomposed(100)), 40);
        assert_eq!(xp_for(&ProgressionEvent::CoderFinished), 20);
        assert_eq!(xp_for(&ProgressionEvent::Integrating), 10);
        assert_eq!(xp_for(&ProgressionEvent::FanoutDone { merged: 1 }), 40);
        assert_eq!(xp_for(&ProgressionEvent::FanoutDone { merged: 0 }), 10);
        assert_eq!(xp_for(&ProgressionEvent::WorkerKilled), 5);
        assert_eq!(xp_for(&ProgressionEvent::ZenTick), 2);
        assert_eq!(xp_for(&ProgressionEvent::PomodoroCompleted), 15);
    }

    #[test]
    fn badge_conditions_each_unlock() {
        fn assert_badge_at(counters: Progression, badge: BadgeId) {
            assert!(counters.badges_for().contains(&badge), "missing {badge:?}");
        }
        assert_badge_at(
            Progression {
                prompts: 1,
                ..Progression::default()
            },
            BadgeId::FirstPrompt,
        );
        assert_badge_at(
            Progression {
                fanouts_done: 1,
                ..Progression::default()
            },
            BadgeId::FirstFanout,
        );
        assert_badge_at(
            Progression {
                seals: 1,
                ..Progression::default()
            },
            BadgeId::FirstSeal,
        );
        assert_badge_at(
            Progression {
                max_workers: 5,
                ..Progression::default()
            },
            BadgeId::Quintet,
        );
        assert_badge_at(
            Progression {
                timed_out_seen: 1,
                ..Progression::default()
            },
            BadgeId::GhostWhisperer,
        );
        assert_badge_at(
            Progression {
                zen_seconds: 300,
                ..Progression::default()
            },
            BadgeId::ZenBath,
        );
        assert_badge_at(
            Progression {
                seals: 10,
                ..Progression::default()
            },
            BadgeId::SealHoarder,
        );
        assert_badge_at(
            Progression {
                prompts: 100,
                ..Progression::default()
            },
            BadgeId::Century,
        );
        // None of the counters set → no badges at all.
        assert!(Progression::default().badges_for().is_empty());
    }

    #[test]
    fn award_levels_up_and_reports_unlocks() {
        let mut p = Progression::default();
        let a = p.award(ProgressionEvent::PromptSubmitted);
        assert_eq!(a.xp, 10);
        assert_eq!(a.level_up, Some((1, "Spark")));
        assert_eq!(a.unlocked, vec![BadgeId::FirstPrompt]);

        // Eight more prompts stay in Spark (90 xp total).
        for _ in 0..8 {
            let a = p.award(ProgressionEvent::PromptSubmitted);
            assert_eq!(a.level_up, None);
            assert!(a.unlocked.is_empty());
        }
        assert_eq!(p.xp, 90);
        assert_eq!(p.level, 1);

        // The award that crosses into Quant reports the level-up.
        let a = p.award(ProgressionEvent::PromptSubmitted);
        assert_eq!(a.xp, 10);
        assert_eq!(a.level_up, Some((2, "Quant")));
        assert_eq!(p.prompts, 10);
        assert_eq!(p.level, 2);
    }

    #[test]
    fn award_fanout_done_branches() {
        let mut p = Progression::default();
        let a = p.award(ProgressionEvent::FanoutDone { merged: 3 });
        assert_eq!(a.xp, 40);
        assert_eq!(p.fanouts_done, 1);
        assert_eq!(p.seals, 3);
        assert!(p.badges.contains(&BadgeId::FirstFanout));
        assert!(p.badges.contains(&BadgeId::FirstSeal));

        let a = p.award(ProgressionEvent::FanoutDone { merged: 0 });
        assert_eq!(a.xp, 10);
        assert_eq!(p.fanouts_done, 2);
        assert_eq!(p.seals, 3, "a zero-merge fan-out adds no seals");
    }

    #[test]
    fn next_badge_label_walks_the_table_then_max() {
        let mut p = Progression::default();
        assert_eq!(p.next_badge_label(), "🜂 First Flame");
        p.badges.push(BadgeId::FirstPrompt);
        assert_eq!(p.next_badge_label(), "🕊 First Flight");
        p.badges = BADGES.iter().map(|(b, _, _)| *b).collect();
        assert_eq!(p.next_badge_label(), "max");
        assert_eq!(badge_name(BadgeId::ZenBath), "Zen Bath");
    }

    #[test]
    fn progression_path_is_config_entheai() {
        let path = progression_path();
        assert!(
            path.ends_with("entheai/progression.json"),
            "unexpected path: {}",
            path.display()
        );
    }

    #[test]
    fn progression_round_trips_through_json() {
        let p = Progression {
            prompts: 42,
            seals: 7,
            badges: vec![BadgeId::FirstPrompt, BadgeId::FirstFanout],
            ..Progression::default()
        };
        let raw = serde_json::to_string(&p).unwrap();
        let back: Progression = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.prompts, 42);
        assert_eq!(back.seals, 7);
        assert_eq!(back.fanouts_done, 0);
        assert_eq!(back.badges, p.badges);
        // The badge enum serializes PascalCase on disk.
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["badges"][0], serde_json::json!("FirstPrompt"));
    }
}
