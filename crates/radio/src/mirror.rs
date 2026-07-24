//! "Mirror in F — Fable's seed": an infinite, deterministic tintinnabuli
//! piece, synthesized sample by sample. No recording, no file, no network —
//! the STYLE is the seed, in Arvo Pärt's grammar but with an original line.
//!
//! Tintinnabuli in two rules (the whole emotion is the algorithm):
//!   * the **M-voice** walks stepwise on the F-major scale, one long tone at
//!     a time, its direction drawn from a seeded RNG — every listener hears
//!     the same eternal walk;
//!   * the **T-voice** answers each M tone with the nearest note of the
//!     F-major triad — strictly below on even steps, strictly above on odd
//!     ones (the classic alternation), so the bell never leaves home.
//!
//! Under both, a quiet quaver ostinato arpeggiates F–A–C, and every eighth
//! M-note is silence: the piece breathes.
//!
//! The generator is pure `std` math (headless-testable); only the
//! `rodio::Source` impl sits behind the `audio` feature.

/// b"FABLE" as the seed — the name made audible.
pub const FABLE_SEED: u64 = 0x0046_4142_4C45;

/// Samples per second (mono).
const RATE: u32 = 44_100;
/// One crotchet at 66 bpm, in samples.
const BEAT: u64 = 40_091;
/// One quaver (the arpeggio pulse).
const EIGHTH: u64 = BEAT / 2;
/// One M-voice tone: four beats.
const M_NOTE: u64 = BEAT * 4;
/// Every Nth M-note is a rest — the breath.
const BREATH_EVERY: u64 = 8;

/// F-major scale, F3..F5.
const SCALE: [f32; 15] = [
    174.61, 196.00, 220.00, 233.08, 261.63, 293.66, 329.63, 349.23, 392.00, 440.00, 466.16, 523.25,
    587.33, 659.26, 698.46,
];
/// The tintinnabuli home: F-major triad tones across the register.
const TRIAD: [f32; 7] = [174.61, 220.00, 261.63, 349.23, 440.00, 523.25, 698.46];
/// The quaver ostinato cycle (F3 A3 C4 A3).
const ARP: [f32; 4] = [174.61, 220.00, 261.63, 220.00];

/// M-voice walk bounds on [`SCALE`] indices (C4..F5 — stays above the arp).
const M_LO: i32 = 4;
const M_HI: i32 = 14;
/// Walk start: A4.
const M_START: i32 = 9;

/// Voice amplitudes (they sum well under clipping).
const AMP_ARP: f32 = 0.055;
const AMP_M: f32 = 0.10;
const AMP_M_HARM: f32 = 0.025;
const AMP_T: f32 = 0.05;

/// M/T envelope: slow bow-stroke attack and release, in samples.
const MT_ATTACK: u64 = 26_460; // 0.6 s
const MT_RELEASE: u64 = 35_280; // 0.8 s
/// Arp envelope: soft hammer, then decay toward (not to) silence.
const ARP_ATTACK: u64 = 220; // 5 ms
const ARP_FLOOR: f32 = 0.15;

/// xorshift64 — tiny, deterministic, and all it has to do is choose a
/// direction. Seed 0 is remapped so the stream never sticks.
struct Walk(u64);
impl Walk {
    fn new(seed: u64) -> Self {
        Walk(if seed == 0 { FABLE_SEED } else { seed })
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// The infinite piece. Pull samples with [`Iterator::next`] (never `None`).
pub struct MirrorInF {
    t: u64,
    rng: Walk,
    /// Which M-note the state below belongs to (`u64::MAX` = not yet begun).
    m_note: u64,
    m_deg: i32,
    m_freq: f32,
    t_freq: f32,
    m_rest: bool,
    // Free-running phases — envelopes shape the notes, phases never reset.
    ph_arp: f32,
    ph_m: f32,
    ph_t: f32,
}

impl MirrorInF {
    pub fn new(seed: u64) -> Self {
        MirrorInF {
            t: 0,
            rng: Walk::new(seed),
            m_note: u64::MAX,
            m_deg: M_START,
            m_freq: SCALE[M_START as usize],
            t_freq: TRIAD[3],
            m_rest: false,
            ph_arp: 0.0,
            ph_m: 0.0,
            ph_t: 0.0,
        }
    }

    /// Nearest triad tone strictly below (or strictly above) `f`; falls back
    /// to the closest tone when the register runs out.
    fn tintinnabuli(f: f32, below: bool) -> f32 {
        let pick = if below {
            TRIAD.iter().rev().find(|&&t| t < f * 0.999)
        } else {
            TRIAD.iter().find(|&&t| t > f * 1.001)
        };
        *pick.unwrap_or_else(|| {
            TRIAD
                .iter()
                .min_by(|a, b| (*a - f).abs().partial_cmp(&(*b - f).abs()).unwrap())
                .expect("triad is non-empty")
        })
    }

    /// Enter M-note `k`: advance the walk (even through rests — silence still
    /// moves), reflect at the register bounds, resolve both voices.
    fn begin_m_note(&mut self, k: u64) {
        self.m_note = k;
        if k > 0 {
            let step = if self.rng.next() & 1 == 0 { 1 } else { -1 };
            self.m_deg += step;
            if self.m_deg > M_HI {
                self.m_deg = M_HI - 1;
            }
            if self.m_deg < M_LO {
                self.m_deg = M_LO + 1;
            }
        }
        self.m_rest = k % BREATH_EVERY == BREATH_EVERY - 1;
        self.m_freq = SCALE[self.m_deg as usize];
        self.t_freq = Self::tintinnabuli(self.m_freq, k.is_multiple_of(2));
    }

    /// Bow-stroke envelope inside a note of length `len` at position `pos`.
    fn mt_env(pos: u64, len: u64) -> f32 {
        if pos < MT_ATTACK {
            pos as f32 / MT_ATTACK as f32
        } else if pos + MT_RELEASE > len {
            (len - pos) as f32 / MT_RELEASE as f32
        } else {
            1.0
        }
    }

    /// One mono sample.
    pub fn next_sample(&mut self) -> f32 {
        use std::f32::consts::TAU;
        let k = self.t / M_NOTE;
        if k != self.m_note {
            self.begin_m_note(k);
        }
        // Arp: which quaver, and how far into it.
        let arp_freq = ARP[(self.t / EIGHTH) as usize % ARP.len()];
        let arp_pos = self.t % EIGHTH;
        let arp_env = if arp_pos < ARP_ATTACK {
            arp_pos as f32 / ARP_ATTACK as f32
        } else {
            let decay = (arp_pos - ARP_ATTACK) as f32 / (EIGHTH - ARP_ATTACK) as f32;
            1.0 - (1.0 - ARP_FLOOR) * decay
        };
        let mut s = AMP_ARP * arp_env * self.ph_arp.sin();

        if !self.m_rest {
            let env = Self::mt_env(self.t % M_NOTE, M_NOTE);
            s += env * (AMP_M * self.ph_m.sin() + AMP_M_HARM * (2.0 * self.ph_m).sin());
            s += env * AMP_T * self.ph_t.sin();
        }

        self.ph_arp = (self.ph_arp + TAU * arp_freq / RATE as f32) % TAU;
        self.ph_m = (self.ph_m + TAU * self.m_freq / RATE as f32) % TAU;
        self.ph_t = (self.ph_t + TAU * self.t_freq / RATE as f32) % TAU;
        self.t += 1;
        s
    }
}

impl Iterator for MirrorInF {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        Some(self.next_sample())
    }
}

#[cfg(feature = "audio")]
impl rodio::Source for MirrorInF {
    fn current_frame_len(&self) -> Option<usize> {
        None // one endless frame
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None // the walk does not end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_is_the_piece_and_a_different_seed_is_a_different_piece() {
        let a: Vec<f32> = MirrorInF::new(FABLE_SEED).take(20_000).collect();
        let b: Vec<f32> = MirrorInF::new(FABLE_SEED).take(20_000).collect();
        assert_eq!(a, b, "same seed must be the identical eternal walk");
        // Walks only diverge at note boundaries — listen past a few of them.
        let c: Vec<f32> = MirrorInF::new(12_345).take((M_NOTE * 4) as usize).collect();
        let a4: Vec<f32> = MirrorInF::new(FABLE_SEED)
            .take((M_NOTE * 4) as usize)
            .collect();
        assert_ne!(a4, c, "a different seed must walk differently");
    }

    #[test]
    fn amplitude_stays_gentle_and_the_piece_is_audible() {
        let mut peak = 0.0_f32;
        let mut energy = 0.0_f64;
        let mut gen = MirrorInF::new(FABLE_SEED);
        let n = 400_000;
        for _ in 0..n {
            let s = gen.next_sample();
            peak = peak.max(s.abs());
            energy += (s as f64) * (s as f64);
        }
        let rms = (energy / n as f64).sqrt();
        assert!(peak < 0.35, "ambient means gentle: peak {peak}");
        assert!(rms > 0.01, "but not silence: rms {rms}");
    }

    #[test]
    fn every_eighth_note_breathes() {
        // Mid-window peaks: a breath (M-note 7) carries only the quiet arp;
        // a sung note (M-note 2) carries all three voices.
        let peak_in = |from: u64, to: u64| {
            let mut g = MirrorInF::new(FABLE_SEED);
            let mut peak = 0.0_f32;
            for i in 0..to {
                let s = g.next_sample();
                if i >= from {
                    peak = peak.max(s.abs());
                }
            }
            peak
        };
        let margin = M_NOTE / 4;
        let sung = peak_in(2 * M_NOTE + margin, 3 * M_NOTE - margin);
        let breath = peak_in(7 * M_NOTE + margin, 8 * M_NOTE - margin);
        assert!(
            breath < 0.08,
            "the rest must be near-silence over the arp: {breath}"
        );
        assert!(sung > breath * 1.5, "sung {sung} vs breath {breath}");
    }

    #[test]
    fn the_walk_never_leaves_the_scale_register() {
        // Drive many note boundaries; the degree must stay in [M_LO, M_HI].
        let mut g = MirrorInF::new(FABLE_SEED);
        for _ in 0..200 {
            for _ in 0..M_NOTE {
                g.next_sample();
            }
            assert!((M_LO..=M_HI).contains(&g.m_deg), "degree {}", g.m_deg);
            assert!(SCALE.contains(&g.m_freq));
        }
    }

    #[test]
    fn the_t_voice_stays_home_in_the_triad() {
        let mut g = MirrorInF::new(FABLE_SEED);
        for _ in 0..64 {
            for _ in 0..M_NOTE {
                g.next_sample();
            }
            assert!(TRIAD.contains(&g.t_freq), "t-voice left the triad");
            // And it mirrors the M-voice from the correct side, alternating.
            if !TRIAD.contains(&g.m_freq) {
                if g.m_note.is_multiple_of(2) {
                    assert!(g.t_freq < g.m_freq, "even steps answer from below");
                } else {
                    assert!(g.t_freq > g.m_freq, "odd steps answer from above");
                }
            }
        }
    }
}
