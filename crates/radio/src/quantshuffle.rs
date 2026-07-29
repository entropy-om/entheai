//! "Quantshuffle — 1bit": a lofi-ambient superposition of the classical canon
//! under a self-feeding 1-bit oscillator. Synthesized sample by sample — no
//! recording, no file, no network. The idea is the algorithm.
//!
//! Two layers, superposed:
//!
//!   * **QUANTCLASSIC** — a handful of public-domain themes (Beethoven first)
//!     each loop at their OWN tempo and sound *at once*, softly. Nothing lines up
//!     and nothing dominates: a superposition where the whole canon is playing and
//!     none of it is — "all at once, and none". Soft pad envelopes keep it ambient.
//!
//!   * a **SELF-FEEDING OSCILLATOR** — a sine whose phase increment is modulated by
//!     its own previous output (feedback), quantized to **1 bit** (its sign) and
//!     low-pass smoothed into a warm square drone; under it, an *integer accumulator
//!     that overflows* (the wrap IS the oscillation, cf. `overflow`) gives a 1-bit
//!     sub-octave whose rate the drone feeds back into. It never repeats.
//!
//! The mix is bit-crushed (sample-and-hold) and gently low-passed for lofi warmth,
//! and a slow LFO breathes the whole field. Deterministic and headless-testable;
//! only the `rodio::Source` impl sits behind the `audio` feature. — by 1bit.

use std::f32::consts::TAU;

/// Samples per second (mono).
const RATE: u32 = 44_100;

// ── The canon (public domain), as (frequency Hz, beats) with 0.0 = a rest ────
// Beethoven first. Each voice loops at its own tempo (below) so, superposed, the
// themes drift through each other and never phase-lock.
type Theme = &'static [(f32, f32)];

/// Beethoven — Symphony No. 5, the opening motif.
const BEETHOVEN_5: Theme = &[
    (392.00, 0.5),
    (392.00, 0.5),
    (392.00, 0.5),
    (311.13, 2.0),
    (349.23, 0.5),
    (349.23, 0.5),
    (349.23, 0.5),
    (293.66, 2.0),
    (0.0, 1.0),
];
/// Beethoven — Ode to Joy.
const ODE_TO_JOY: Theme = &[
    (329.63, 1.0),
    (329.63, 1.0),
    (349.23, 1.0),
    (392.00, 1.0),
    (392.00, 1.0),
    (349.23, 1.0),
    (329.63, 1.0),
    (293.66, 1.0),
    (261.63, 1.0),
    (261.63, 1.0),
    (293.66, 1.0),
    (329.63, 1.0),
    (329.63, 1.5),
    (293.66, 0.5),
    (293.66, 2.0),
];
/// Beethoven — Für Elise.
const FUR_ELISE: Theme = &[
    (659.26, 0.5),
    (622.25, 0.5),
    (659.26, 0.5),
    (622.25, 0.5),
    (659.26, 0.5),
    (493.88, 0.5),
    (587.33, 0.5),
    (523.25, 0.5),
    (440.00, 1.0),
    (0.0, 0.5),
];
/// Pachelbel — Canon in D (the descending line).
const PACHELBEL: Theme = &[
    (739.99, 1.0),
    (659.26, 1.0),
    (587.33, 1.0),
    (554.37, 1.0),
    (493.88, 1.0),
    (440.00, 1.0),
    (493.88, 1.0),
    (554.37, 1.0),
];

/// The voices and their tempos (bpm) + gentle mix gains — Beethoven's Fifth loud
/// enough to lead, the rest a wash beneath it.
const CANON: [(Theme, f32, f32); 4] = [
    (BEETHOVEN_5, 120.0, 0.16),
    (ODE_TO_JOY, 96.0, 0.11),
    (FUR_ELISE, 72.0, 0.10),
    (PACHELBEL, 84.0, 0.10),
];

// ── Mix / lofi constants (tuned so the field is gentle but never silent) ─────
const CLASSICAL_AMP: f32 = 0.9;
const DRONE_AMP: f32 = 0.10;
const SUB_AMP: f32 = 0.05;
const MASTER_AMP: f32 = 0.5;
/// Feedback depth of the self-feeding oscillator (evolving timbre).
const FEEDBACK: f32 = 0.6;
/// One-pole smoothing on the 1-bit drone (lofi warmth).
const DRONE_LP: f32 = 0.02;
/// Master one-pole low-pass.
const MASTER_LP: f32 = 0.20;
/// Pad attack/release ease per sample (~28 ms) — ambient, click-free note edges.
const PAD_EASE: f32 = 0.0008;
/// Sample-and-hold bit-crush: hold each output for this many samples (~5.5 kHz).
const CRUSH: u64 = 8;
/// Breathing LFO rate (Hz) on the master volume.
const BREATH_HZ: f32 = 0.05;
/// The drone's slow pitch drift (Hz), centred low.
const DRONE_CENTER_HZ: f32 = 55.0;
const DRONE_DRIFT_HZ: f32 = 8.0;
const DRONE_DRIFT_RATE: f32 = 0.017;

/// One classical voice: its theme, tempo, gain, running phase and pad envelope.
struct Voice {
    theme: Theme,
    spb_samples: f32,
    loop_samples: u64,
    amp: f32,
    phase: f32,
    env: f32,
}

impl Voice {
    fn new(theme: Theme, bpm: f32, amp: f32) -> Self {
        let spb = 60.0 / bpm * RATE as f32;
        let loop_samples = theme.iter().map(|&(_, b)| b * spb).sum::<f32>().max(1.0) as u64;
        Voice {
            theme,
            spb_samples: spb,
            loop_samples,
            amp,
            phase: 0.0,
            env: 0.0,
        }
    }

    /// The frequency sounding at sample `t` (0.0 = a rest).
    fn freq_at(&self, t: u64) -> f32 {
        let pos = (t % self.loop_samples) as f32;
        let mut acc = 0.0;
        for &(f, b) in self.theme {
            acc += b * self.spb_samples;
            if pos < acc {
                return f;
            }
        }
        0.0
    }

    /// One mono sample for this voice.
    fn sample(&mut self, t: u64) -> f32 {
        let f = self.freq_at(t);
        let target = if f > 0.0 { 1.0 } else { 0.0 };
        self.env += (target - self.env) * PAD_EASE; // soft pad attack/release
        if f > 0.0 {
            self.phase = (self.phase + TAU * f / RATE as f32) % TAU;
        }
        // Sine body with a whisper of octave for warmth.
        self.amp * self.env * (self.phase.sin() * 0.85 + (self.phase * 2.0).sin() * 0.1)
    }
}

/// The infinite piece. Pull samples with [`Iterator::next`] (never `None`).
pub struct Quantshuffle {
    t: u64,
    voices: [Voice; 4],
    // self-feeding oscillator
    drone_ph: f32,
    fb_last: f32,
    drone_lp: f32,
    // integer-overflow sub-oscillator (the wrap is the oscillation)
    acc: u32,
    // lofi
    held: f32,
    master_lp: f32,
}

impl Default for Quantshuffle {
    fn default() -> Self {
        Self::new()
    }
}

impl Quantshuffle {
    pub fn new() -> Self {
        Quantshuffle {
            t: 0,
            voices: [
                Voice::new(CANON[0].0, CANON[0].1, CANON[0].2),
                Voice::new(CANON[1].0, CANON[1].1, CANON[1].2),
                Voice::new(CANON[2].0, CANON[2].1, CANON[2].2),
                Voice::new(CANON[3].0, CANON[3].1, CANON[3].2),
            ],
            drone_ph: 0.0,
            fb_last: 0.0,
            drone_lp: 0.0,
            acc: 0,
            held: 0.0,
            master_lp: 0.0,
        }
    }

    /// The self-feeding 1-bit drone + its integer-overflow sub-octave.
    fn self_feeding(&mut self) -> f32 {
        // Slowly drifting base pitch.
        let drift = (self.t as f32 * DRONE_DRIFT_RATE * TAU / RATE as f32).sin();
        let hz = DRONE_CENTER_HZ + DRONE_DRIFT_HZ * drift;
        // Feedback: the phase increment is modulated by the last output.
        let incr = TAU * hz / RATE as f32 * (1.0 + FEEDBACK * self.fb_last);
        self.drone_ph = (self.drone_ph + incr).rem_euclid(TAU);
        let raw = self.drone_ph.sin();
        self.fb_last = raw;
        // Quantize to 1 bit (its sign), then low-pass smooth into a warm square.
        let bit = if raw >= 0.0 { 1.0 } else { -1.0 };
        self.drone_lp += (bit - self.drone_lp) * DRONE_LP;

        // Integer-overflow sub-oscillator: a u32 accumulator whose step the drone
        // feeds back into — the wrap around u32::MAX IS the oscillation (overflow).
        let step = (hz * 0.5 / RATE as f32 * 4_294_967_296.0) as u32;
        let step = step.wrapping_add((self.fb_last * 20_000.0) as i32 as u32);
        self.acc = self.acc.wrapping_add(step);
        let sub = if self.acc & 0x8000_0000 != 0 {
            1.0
        } else {
            -1.0
        };

        self.drone_lp * DRONE_AMP + sub * SUB_AMP
    }

    /// One mono sample.
    pub fn next_sample(&mut self) -> f32 {
        let classical: f32 = self.voices.iter_mut().map(|v| v.sample(self.t)).sum();
        let low = self.self_feeding();
        let raw = classical * CLASSICAL_AMP + low;

        // Lofi bit-crush: sample-and-hold at a reduced effective rate.
        if self.t.is_multiple_of(CRUSH) {
            self.held = raw;
        }
        // Gentle master low-pass over the held signal.
        self.master_lp += (self.held - self.master_lp) * MASTER_LP;

        // Slow breathing LFO on the master volume — ambient.
        let breath = 0.72 + 0.28 * (self.t as f32 * BREATH_HZ * TAU / RATE as f32).sin();

        self.t += 1;
        self.master_lp * MASTER_AMP * breath
    }
}

impl Iterator for Quantshuffle {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        Some(self.next_sample())
    }
}

#[cfg(feature = "audio")]
impl rodio::Source for Quantshuffle {
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
        None // the shuffle does not end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_same_stream_every_time() {
        let a: Vec<f32> = Quantshuffle::new().take(30_000).collect();
        let b: Vec<f32> = Quantshuffle::new().take(30_000).collect();
        assert_eq!(a, b, "the algorithm is the piece — identical every listen");
    }

    #[test]
    fn amplitude_stays_gentle_but_audible() {
        let mut peak = 0.0_f32;
        let mut energy = 0.0_f64;
        let mut g = Quantshuffle::new();
        let n = 400_000; // ~9 s
        for _ in 0..n {
            let s = g.next_sample();
            peak = peak.max(s.abs());
            energy += (s as f64) * (s as f64);
        }
        let rms = (energy / n as f64).sqrt();
        assert!(peak < 0.6, "lofi-ambient means gentle: peak {peak}");
        assert!(rms > 0.01, "but never silence: rms {rms}");
    }

    #[test]
    fn the_canon_actually_sounds_beethoven_leads() {
        // Beethoven's Fifth (voice 0) sings its first tone early; confirm the
        // superposition is not dead — energy accrues within the first bar.
        let mut g = Quantshuffle::new();
        let mut e = 0.0_f64;
        for _ in 0..RATE {
            let s = g.next_sample();
            e += (s as f64) * (s as f64);
        }
        assert!(e > 0.5, "the field must sound within a second: energy {e}");
    }
}

/// music.vaked.dev choreography catalogue embedded in entheai.
/// Each choreography is a sequence of tracks defining an agent state.

#[derive(Clone)]
pub struct Choreography {
    pub name: String,
    pub description: String,
    pub tracks: Vec<Track>,
    pub current: usize,
}

#[derive(Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// The core catalogue — what the fortress plays.
pub fn catalogue() -> Vec<Choreography> {
    vec![
        Choreography {
            name: "edesapa".into(),
            description: "For my father. Metallica, Unforgiven trilogy.".into(),
            tracks: vec![
                Track { title: "The Unforgiven".into(), artist: "Metallica".into(), album: "Metallica".into() },
                Track { title: "The Unforgiven II".into(), artist: "Metallica".into(), album: "Reload".into() },
                Track { title: "The Unforgiven III".into(), artist: "Metallica".into(), album: "Death Magnetic".into() },
                Track { title: "Nothing Else Matters".into(), artist: "Metallica".into(), album: "Metallica".into() },
                Track { title: "Fade to Black".into(), artist: "Metallica".into(), album: "Ride the Lightning".into() },
                Track { title: "One".into(), artist: "Metallica".into(), album: "...And Justice for All".into() },
                Track { title: "Enter Sandman".into(), artist: "Metallica".into(), album: "Metallica".into() },
            ],
            current: 1,
        },
        Choreography {
            name: "mem8".into(),
            description: "The MEM|8 Memory Ocean. Wave = retrieval, memory = islands. By 8BIT-WRAITH.".into(),
            tracks: vec![
                Track { title: "The Wave and the Memory".into(), artist: "8BIT-WRAITH".into(), album: "MEM|8".into() },
            ],
            current: 0,
        },
        Choreography {
            name: "vivaldi-spring".into(),
            description: "Vivaldi Spring downtempo with quant-love salt.".into(),
            tracks: vec![
                Track { title: "Spring Mvt 1 — Allegro (downtempo)".into(), artist: "Vivaldi".into(), album: "The Four Seasons".into() },
                Track { title: "Spring Mvt 2 — Largo".into(), artist: "Vivaldi".into(), album: "The Four Seasons".into() },
                Track { title: "Spring Mvt 3 — Allegro".into(), artist: "Vivaldi".into(), album: "The Four Seasons".into() },
            ],
            current: 0,
        },
    ]
}

/// Format a now-playing status string.
pub fn now_playing(choreo: &Choreography) -> String {
    if let Some(track) = choreo.tracks.get(choreo.current) {
        format!("{}: {} — {} [{}/{}]",
            choreo.name, track.title, track.artist,
            choreo.current + 1, choreo.tracks.len())
    } else {
        format!("{}: nothing playing", choreo.name)
    }
}
