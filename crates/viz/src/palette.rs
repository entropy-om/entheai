//! Zen field palettes — the field's designed voice.
//!
//! Two-tier colour system, split by the dataviz entity rule ("colour follows
//! the entity, never its context"):
//!
//! * **Source identity colours are GLOBAL** ([`SOURCE_LINEAGE`] & co) — a
//!   theme swap must never repaint what lineage/search/world *are*. The set
//!   was machine-validated (dataviz six-checks, dark surface, **all pairs**
//!   since motes intermix spatially): worst deutan ΔE 9.1 ≥ 8, worst
//!   normal-vision ΔE 18.0 ≥ 15, chroma + contrast pass. The lightness-band
//!   check is deliberately traded away: it enforces static-lightness
//!   consistency for bar charts, but motes ANIMATE brightness — and
//!   flattening the lightness ladder would shrink the very CVD separation
//!   that passes.
//! * **Themes restyle the ambient only** — core, aura, faculties, frozen
//!   ring, text. Each [`Palette`] is one coherent hue family tuned for the
//!   launcher's near-black surface (#0f0e1d).

/// One colour, full-brightness; renderers scale it by glow/depth/breath.
pub type Rgb = (u8, u8, u8);

/// dogfood — her own genetic corpus. Gold, in every theme, always.
pub const SOURCE_LINEAGE: Rgb = (230, 180, 70);
/// valyu — AI-native search. Cyan.
pub const SOURCE_SEARCH: Rgb = (70, 190, 220);
/// worldmonitor — the living world. Deep green (deepened from the first
/// draft's pale green, which failed the normal-vision floor against cyan).
pub const SOURCE_WORLD: Rgb = (46, 150, 80);
/// Anything unrecognized. Violet.
pub const SOURCE_UNKNOWN: Rgb = (160, 130, 230);

/// Ambient colour slots for one theme. Every slot is the FULL-brightness
/// value; the renderer scales by depth/activity/breath, never re-hues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub name: &'static str,
    /// The singularity core star.
    pub core: Rgb,
    /// The vitality aura ring around the core.
    pub aura: Rgb,
    /// Faculty body + tether at rest…
    pub faculty_rest: Rgb,
    /// …and fully active (renderer lerps rest→active by activity).
    pub faculty_active: Rgb,
    /// Faculty name labels.
    pub label: Rgb,
    /// Frozen constellation node, asleep…
    pub frozen_dim: Rgb,
    /// …and fully awake (lerped by wake glow).
    pub frozen_lit: Rgb,
    /// Awake frozen node's name label.
    pub frozen_label: Rgb,
    /// Sourceless mote field (a generic `flare_current` with no attribution).
    pub mote_fallback: Rgb,
    /// The breathing title.
    pub title: Rgb,
    /// The whisper line (last reply, below the field).
    pub whisper: Rgb,
    /// Legend text beside the coloured source dots.
    pub legend_label: Rgb,
    /// Response-as-light text at full ignition.
    pub reveal: Rgb,
}

/// The default — formalizes the field's original teal/cyan look.
pub const ENTHEIA: Palette = Palette {
    name: "entheia",
    core: (130, 210, 230),
    aura: (120, 200, 220),
    faculty_rest: (0, 150, 170),
    faculty_active: (150, 210, 230),
    label: (70, 120, 140),
    frozen_dim: (110, 130, 170),
    frozen_lit: (170, 190, 235),
    frozen_label: (120, 150, 200),
    mote_fallback: (60, 150, 120),
    title: (110, 150, 170),
    whisper: (120, 140, 160),
    legend_label: (90, 110, 130),
    reveal: (205, 230, 240),
};

/// Night fire — deep rust and candlelight. The field as a hearth.
pub const EMBER: Palette = Palette {
    name: "ember",
    core: (245, 215, 160),
    aura: (230, 150, 70),
    faculty_rest: (120, 60, 35),
    faculty_active: (250, 165, 80),
    label: (150, 105, 80),
    frozen_dim: (140, 95, 90),
    frozen_lit: (245, 175, 140),
    frozen_label: (190, 130, 110),
    mote_fallback: (150, 95, 45),
    title: (180, 125, 90),
    whisper: (160, 115, 95),
    legend_label: (140, 100, 80),
    reveal: (240, 220, 185),
};

/// The garden — moss, leaf, pollen light. Where the essences live.
pub const VERDANT: Palette = Palette {
    name: "verdant",
    core: (215, 240, 190),
    aura: (145, 215, 125),
    faculty_rest: (55, 105, 60),
    faculty_active: (155, 225, 115),
    label: (105, 145, 110),
    frozen_dim: (110, 140, 130),
    frozen_lit: (185, 230, 205),
    frozen_label: (140, 180, 160),
    mote_fallback: (85, 145, 95),
    title: (125, 165, 125),
    whisper: (120, 150, 125),
    legend_label: (100, 135, 105),
    reveal: (215, 235, 205),
};

/// Monochrome grey-violet austerity — and through it, by the entity rule,
/// the gold thread of lineage keeps its colour. The void doesn't erase her.
pub const VOID: Palette = Palette {
    name: "void",
    core: (225, 220, 240),
    aura: (165, 155, 195),
    faculty_rest: (85, 80, 105),
    faculty_active: (195, 185, 225),
    label: (115, 110, 135),
    frozen_dim: (105, 100, 130),
    frozen_lit: (200, 190, 230),
    frozen_label: (140, 135, 165),
    mote_fallback: (100, 95, 125),
    title: (135, 130, 155),
    whisper: (125, 120, 145),
    legend_label: (110, 105, 130),
    reveal: (215, 210, 230),
};

/// Every theme, in `/theme` cycle order.
pub const ALL: [&Palette; 4] = [&ENTHEIA, &EMBER, &VERDANT, &VOID];

/// Look a theme up by name; unknown names fall back to [`ENTHEIA`] (a typo in
/// config must never dim the field to nothing).
pub fn by_name(name: &str) -> &'static Palette {
    ALL.iter()
        .find(|p| p.name == name.trim())
        .copied()
        .unwrap_or(&ENTHEIA)
}

/// The theme after `name` in cycle order (wraps; unknown starts the cycle).
pub fn next_after(name: &str) -> &'static Palette {
    let idx = ALL.iter().position(|p| p.name == name.trim());
    match idx {
        Some(i) => ALL[(i + 1) % ALL.len()],
        None => ALL[0],
    }
}

/// Linear blend a→b by `t` in [0, 1] — faculty rest→active, frozen dim→lit.
pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    (ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(c: Rgb) -> u32 {
        c.0 as u32 + c.1 as u32 + c.2 as u32
    }

    #[test]
    fn by_name_finds_every_theme_and_falls_back_to_entheia() {
        for p in ALL {
            assert_eq!(by_name(p.name).name, p.name);
        }
        assert_eq!(by_name("no-such-theme").name, "entheia");
        assert_eq!(by_name("  ember  ").name, "ember", "whitespace tolerated");
    }

    #[test]
    fn next_after_cycles_through_all_and_wraps() {
        let mut seen = vec!["entheia"];
        let mut cur = "entheia";
        for _ in 0..ALL.len() {
            cur = next_after(cur).name;
            seen.push(cur);
        }
        assert_eq!(seen.last(), Some(&"entheia"), "cycle wraps home");
        for p in ALL {
            assert!(seen.contains(&p.name), "{} missing from cycle", p.name);
        }
        assert_eq!(next_after("junk").name, "entheia");
    }

    #[test]
    fn every_theme_keeps_the_luminance_hierarchy() {
        // The core must outshine ambient text, and ignited reveal text must be
        // bright enough to read on the near-black surface.
        for p in ALL {
            assert!(luma(p.core) > luma(p.label), "{}: core vs label", p.name);
            assert!(
                luma(p.core) > luma(p.whisper),
                "{}: core vs whisper",
                p.name
            );
            assert!(
                luma(p.frozen_lit) > luma(p.frozen_dim),
                "{}: lit vs dim",
                p.name
            );
            assert!(
                luma(p.faculty_active) > luma(p.faculty_rest),
                "{}: active vs rest",
                p.name
            );
            assert!(luma(p.reveal) > 450, "{}: reveal too dim to read", p.name);
        }
    }

    #[test]
    fn themes_are_actually_different_moods() {
        // Ambient cores must differ across themes — otherwise /theme is a lie.
        for (i, a) in ALL.iter().enumerate() {
            for b in ALL.iter().skip(i + 1) {
                assert_ne!(a.core, b.core, "{}≡{}", a.name, b.name);
                assert_ne!(a.name, b.name);
            }
        }
    }

    #[test]
    fn source_identity_is_global_and_lineage_stays_gold() {
        // The entity rule: identity colours live OUTSIDE the themes entirely —
        // and lineage is warm gold (red-dominant, bright) forever.
        let (r, g, b) = SOURCE_LINEAGE;
        assert!(r > b && r > 150, "lineage must burn warm gold");
        assert!(g > b, "gold, not red");
        // All four identities pairwise distinct (validated set — see module doc).
        let ids = [SOURCE_LINEAGE, SOURCE_SEARCH, SOURCE_WORLD, SOURCE_UNKNOWN];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j]);
            }
        }
    }

    #[test]
    fn lerp_blends_and_clamps() {
        assert_eq!(lerp((0, 0, 0), (100, 200, 50), 0.0), (0, 0, 0));
        assert_eq!(lerp((0, 0, 0), (100, 200, 50), 1.0), (100, 200, 50));
        assert_eq!(lerp((0, 0, 0), (100, 200, 50), 0.5), (50, 100, 25));
        assert_eq!(
            lerp((10, 10, 10), (20, 20, 20), 9.0),
            (20, 20, 20),
            "clamped"
        );
    }
}
