use crate::qr::QrGrid;

/// The companion window draws at this frame rate.
const FPS: f64 = 24.0;

/// Duration of one full breathing cycle in seconds.
const BREATH_PERIOD: f64 = 3.0;

/// Pulse amplitude: glow oscillates between `1.0 - AMPLITUDE` and `1.0 + AMPLITUDE`.
const AMPLITUDE: f64 = 0.5;

/// Colors.
const BG: (u8, u8, u8) = (0x0a, 0x0f, 0x14);
const GLOW: (u8, u8, u8) = (0x00, 0xe5, 0xff);
const QR_DARK: (u8, u8, u8) = (0x2a, 0x4a, 0x55); // teal-dimmed module
const QR_LIGHT: (u8, u8, u8) = (0x0d, 0x18, 0x1f); // slightly brighter than BG

/// Pack BGRA into a little-endian u32 (macOS softbuffer byte order).
#[inline]
fn pack_bgra(b: u8, g: u8, r: u8, a: u8) -> u32 {
    u32::from_le_bytes([b, g, r, a])
}

/// Linear interpolation between two u8 components.
#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)) as u8
}

/// Render one animation frame into a BGRA pixel buffer.
///
/// `time` is seconds since the companion started; it drives the breathing
/// animation. The glow pulses on a `BREATH_PERIOD`-second sine cycle.
pub fn render_frame(buffer: &mut [u32], width: u32, height: u32, qr: &QrGrid, time: f64) {
    let w = width as f32;
    let h = height as f32;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let max_r = w.min(h) / 2.0;

    // Breathing pulse: 0.0 (dim) .. 1.0 (bright), sinusoidal.
    let pulse_raw = (time * std::f64::consts::TAU / BREATH_PERIOD).sin() as f32;
    let pulse = 1.0 - AMPLITUDE as f32 + AMPLITUDE as f32 * pulse_raw;

    // Phase 1: draw the breathing radial glow as background.
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt() / max_r;

            // Soft radial falloff: full glow in center, fading at edges.
            let glow_factor = smooth_falloff(dist);

            // Blend BG → GLOW by the product of glow shape and current pulse.
            let alpha = glow_factor * pulse;
            let (r, g, b) = (
                lerp_u8(BG.0, GLOW.0, alpha),
                lerp_u8(BG.1, GLOW.1, alpha),
                lerp_u8(BG.2, GLOW.2, alpha),
            );
            buffer[(y * width + x) as usize] = pack_bgra(b, g, r, 255);
        }
    }

    // Phase 2: overlay the QR code, centered.
    let qr_px = (w.min(h) * 0.60) as u32; // QR occupies 60% of window
    let module_px = qr_px / qr.size as u32;
    let total_qr_px = module_px * qr.size as u32;
    let qr_x0 = (width - total_qr_px) / 2;
    let qr_y0 = (height - total_qr_px) / 2;

    for my in 0..qr.size {
        for mx in 0..qr.size {
            let dark = qr.is_dark(mx, my);
            let (r, g, b) = if dark { QR_DARK } else { QR_LIGHT };
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
                        buffer[idx] = pack_bgra(b, g, r, 255);
                    }
                }
            }
        }
    }
}

/// Smooth radial falloff: 1.0 at center, transitioning to 0.0 at the edge.
fn smooth_falloff(dist: f32) -> f32 {
    if dist < 0.6 {
        1.0
    } else if dist > 1.1 {
        0.0
    } else {
        let t = (dist - 0.6) / 0.5;
        // Smoothstep: 3t² - 2t³
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

/// How many seconds between frames.
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
        assert_eq!(bytes[0], 0xAB); // B
        assert_eq!(bytes[1], 0xCD); // G
        assert_eq!(bytes[2], 0xEF); // R
        assert_eq!(bytes[3], 0xFF); // A
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
}
