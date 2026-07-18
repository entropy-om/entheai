use crate::qr::QrGrid;
use crate::state::State;

/// Packed return of [`params_for`]: glow color, period, amplitude range,
/// spinner flag, flash alpha.
struct GlowParams((f32, f32, f32), f64, (f32, f32), bool, f32);

const FPS: f64 = 24.0;
const TRANSITION_S: f64 = 0.3;
const FLASH_S: f64 = 0.2;

const BG: (u8, u8, u8) = (0x0a, 0x0f, 0x14);
const TEAL: (u8, u8, u8) = (0x00, 0xe5, 0xff);
const MAGENTA: (u8, u8, u8) = (0xff, 0x00, 0xe5);
const RED: (u8, u8, u8) = (0xff, 0x44, 0x44);
const QR_DARK: (u8, u8, u8) = (0x2a, 0x4a, 0x55);
const QR_LIGHT: (u8, u8, u8) = (0x0d, 0x18, 0x1f);

const GLYPH_QUESTION: [[bool; 5]; 7] = [
    [false, true, true, true, false],
    [true, false, false, false, true],
    [false, false, false, false, true],
    [false, false, false, true, false],
    [false, false, true, false, false],
    [false, false, false, false, false],
    [false, false, true, false, false],
];

pub struct AnimationState {
    target_state: State,
    target_glow: (f32, f32, f32),
    glow: (f32, f32, f32),
    target_period: f64,
    target_amplitude: (f32, f32),
    pub target_spinner: bool,
    target_qr_dim: f32,
    qr_dim: f32,
    flash_until: Option<f64>,
    /// When set, the companion is fading out. Value decreases from 1.0 → 0.0.
    pub fade_alpha: f32,
}

impl Default for AnimationState {
    fn default() -> Self {
        let glow = (TEAL.0 as f32, TEAL.1 as f32, TEAL.2 as f32);
        Self {
            target_state: State::Idle,
            target_glow: glow,
            glow,
            target_period: 3.0,
            target_amplitude: (0.2, 0.6),
            target_spinner: false,
            target_qr_dim: 0.0,
            qr_dim: 0.0,
            flash_until: None,
            fade_alpha: 1.0,
        }
    }
}

impl AnimationState {
    pub fn set_state(&mut self, state: State) {
        if self.target_state == state {
            return;
        }
        self.target_state = state;
        let GlowParams(glow, period, ampl, spinner, qr_dim) = params_for(state);
        self.target_glow = glow;
        self.target_period = period;
        self.target_amplitude = ampl;
        self.target_spinner = spinner;
        self.target_qr_dim = qr_dim;
    }

    pub fn flash(&mut self, now: f64) {
        self.flash_until = Some(now + FLASH_S);
    }

    pub fn tick(&mut self, dt: f64) {
        let t = (dt / TRANSITION_S).clamp(0.0, 1.0) as f32;
        self.glow = (
            lerp_f32(self.glow.0, self.target_glow.0, t),
            lerp_f32(self.glow.1, self.target_glow.1, t),
            lerp_f32(self.glow.2, self.target_glow.2, t),
        );
        self.qr_dim = lerp_f32(self.qr_dim, self.target_qr_dim, t);
    }
}

fn params_for(state: State) -> GlowParams {
    match state {
        State::Idle => GlowParams(
            (TEAL.0 as f32, TEAL.1 as f32, TEAL.2 as f32),
            3.0,
            (0.2, 0.6),
            false,
            0.0,
        ),
        State::Working => GlowParams(
            (TEAL.0 as f32, TEAL.1 as f32, TEAL.2 as f32),
            1.5,
            (0.3, 0.8),
            true,
            0.0,
        ),
        State::PermissionPending => GlowParams(
            (MAGENTA.0 as f32, MAGENTA.1 as f32, MAGENTA.2 as f32),
            1.0,
            (0.4, 1.0),
            false,
            0.6,
        ),
        State::Error => GlowParams(
            (RED.0 as f32, RED.1 as f32, RED.2 as f32),
            4.0,
            (0.1, 0.3),
            false,
            0.7,
        ),
    }
}

#[inline]
fn pack_bgra(b: u8, g: u8, r: u8, a: u8) -> u32 {
    u32::from_le_bytes([b, g, r, a])
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)) as u8
}

#[inline]
fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

pub fn render_frame(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    qr: &QrGrid,
    anim: &AnimationState,
    time: f64,
) {
    let w = width as f32;
    let h = height as f32;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let max_r = w.min(h) / 2.0;

    let (pulse_min, pulse_max) = anim.target_amplitude;
    let pulse_raw = (time * std::f64::consts::TAU / anim.target_period).sin() as f32;
    let pulse = pulse_min + (pulse_max - pulse_min) * (pulse_raw * 0.5 + 0.5);

    let flash_active = anim.flash_until.is_some_and(|f| time < f);
    let pulse = if flash_active { 1.0 } else { pulse };

    let (gr, gg, gb) = anim.glow;
    let fa = anim.fade_alpha;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt() / max_r;
            let glow_factor = smooth_falloff(dist);
            let alpha = glow_factor * pulse;
            let r = lerp_u8(BG.0, gr as u8, alpha);
            let g = lerp_u8(BG.1, gg as u8, alpha);
            let b = lerp_u8(BG.2, gb as u8, alpha);
            let a = (255.0 * fa) as u8;
            buffer[(y * width + x) as usize] = pack_bgra(b, g, r, a);
        }
    }

    let qr_px = (w.min(h) * 0.60) as u32;
    let module_px = qr_px / qr.size as u32;
    let total_qr_px = module_px * qr.size as u32;
    let qr_x0 = (width - total_qr_px) / 2;
    let qr_y0 = (height - total_qr_px) / 2;
    let dim = anim.qr_dim;

    for my in 0..qr.size {
        for mx in 0..qr.size {
            let dark = qr.is_dark(mx, my);
            let (r, g, b) = if dark {
                (
                    lerp_u8(QR_DARK.0, BG.0, dim),
                    lerp_u8(QR_DARK.1, BG.1, dim),
                    lerp_u8(QR_DARK.2, BG.2, dim),
                )
            } else {
                (
                    lerp_u8(QR_LIGHT.0, BG.0, dim),
                    lerp_u8(QR_LIGHT.1, BG.1, dim),
                    lerp_u8(QR_LIGHT.2, BG.2, dim),
                )
            };
            let base_x = qr_x0 + mx as u32 * module_px;
            let base_y = qr_y0 + my as u32 * module_px;
            for dy in 0..module_px {
                let py = base_y + dy;
                if py >= height {
                    break;
                }
                let row_start = (py * width + base_x) as usize;
                for dx in 0..module_px {
                    let idx = row_start + dx as usize;
                    if idx < buffer.len() {
                        buffer[idx] = pack_bgra(b, g, r, (255.0 * fa) as u8);
                    }
                }
            }
        }
    }

    if anim.target_spinner {
        draw_spinner(buffer, width, height, cx, cy, time, fa);
    }

    if anim.target_state == State::PermissionPending {
        draw_glyph(
            buffer,
            width,
            height,
            &GLYPH_QUESTION,
            cx,
            cy,
            MAGENTA,
            pulse * fa,
        );
    }
}

fn draw_spinner(buffer: &mut [u32], w: u32, h: u32, cx: f32, cy: f32, time: f64, fade_alpha: f32) {
    let radius = w.min(h) as f32 * 0.55;
    let angle = time as f32 * std::f32::consts::TAU / 2.0;
    let sx = cx + radius * angle.cos();
    let sy = cy + radius * angle.sin();
    let dot_r = 2i32;
    let a = (255.0 * fade_alpha) as u8;
    for dy in -dot_r..=dot_r {
        for dx in -dot_r..=dot_r {
            if dx * dx + dy * dy > dot_r * dot_r {
                continue;
            }
            let px = (sx + dx as f32) as i32;
            let py = (sy + dy as f32) as i32;
            if px >= 0 && px < w as i32 && py >= 0 && py < h as i32 {
                let idx = (py as u32 * w + px as u32) as usize;
                if idx < buffer.len() {
                    buffer[idx] = pack_bgra(TEAL.2, TEAL.1, TEAL.0, a);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_glyph(
    buffer: &mut [u32],
    w: u32,
    h: u32,
    glyph: &[[bool; 5]; 7],
    cx: f32,
    cy: f32,
    color: (u8, u8, u8),
    alpha: f32,
) {
    let scale = 3u32;
    let gw = 5 * scale;
    let gh = 7 * scale;
    let x0 = (cx as u32).saturating_sub(gw / 2);
    let y0 = (cy as u32).saturating_sub(gh / 2);
    for row in 0..7u32 {
        for col in 0..5u32 {
            if !glyph[row as usize][col as usize] {
                continue;
            }
            let base_x = x0 + col * scale;
            let base_y = y0 + row * scale;
            for dy in 0..scale {
                let py = base_y + dy;
                if py >= h {
                    break;
                }
                for dx in 0..scale {
                    let px = base_x + dx;
                    if px >= w {
                        break;
                    }
                    let idx = (py * w + px) as usize;
                    if idx < buffer.len() {
                        let existing = buffer[idx];
                        let eb = (existing & 0xFF) as u8;
                        let eg = ((existing >> 8) & 0xFF) as u8;
                        let er = ((existing >> 16) & 0xFF) as u8;
                        let r = lerp_u8(er, color.0, alpha);
                        let g = lerp_u8(eg, color.1, alpha);
                        let b = lerp_u8(eb, color.2, alpha);
                        buffer[idx] = pack_bgra(b, g, r, 255);
                    }
                }
            }
        }
    }
}

fn smooth_falloff(dist: f32) -> f32 {
    if dist < 0.6 {
        1.0
    } else if dist > 1.1 {
        0.0
    } else {
        let t = (dist - 0.6) / 0.5;
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

pub fn frame_interval() -> std::time::Duration {
    std::time::Duration::from_secs_f64(1.0 / FPS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_bgra() {
        let pixel = pack_bgra(0xAB, 0xCD, 0xEF, 0xFF);
        let bytes = pixel.to_le_bytes();
        assert_eq!(bytes[0], 0xAB);
        assert_eq!(bytes[1], 0xCD);
        assert_eq!(bytes[2], 0xEF);
        assert_eq!(bytes[3], 0xFF);
    }

    #[test]
    fn test_lerp_u8() {
        assert_eq!(lerp_u8(0, 100, 0.5), 50);
        assert_eq!(lerp_u8(0, 255, 1.0), 255);
        assert_eq!(lerp_u8(0, 255, 0.0), 0);
    }

    #[test]
    fn test_smooth_falloff() {
        assert!((smooth_falloff(0.0) - 1.0).abs() < 0.01);
        assert!((smooth_falloff(0.3) - 1.0).abs() < 0.01);
        assert!(smooth_falloff(0.85) < 1.0 && smooth_falloff(0.85) > 0.0);
        assert!((smooth_falloff(1.2) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_params_for_each_state() {
        for state in &[
            State::Idle,
            State::Working,
            State::PermissionPending,
            State::Error,
        ] {
            let GlowParams(..) = params_for(*state);
        }
    }

    #[test]
    fn test_animation_state_transitions() {
        let mut anim = AnimationState::default();
        anim.set_state(State::Working);
        assert!(anim.target_spinner);
        anim.tick(0.15);
        anim.tick(0.15);
        assert!((anim.glow.0 - TEAL.0 as f32).abs() < 1.0);
    }
}
