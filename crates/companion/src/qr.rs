use qrcode::QrCode;
use serde::{Deserialize, Serialize};

/// The payload encoded in the companion QR code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPayload {
    /// Schema version.
    pub v: u8,
    /// Session UUID.
    pub sid: String,
    /// Tailscale MagicDNS hostname or `hostname.local`.
    pub host: String,
    /// Port for the remote session endpoint (future: comms crate).
    pub port: u16,
    /// Working directory the session was started from.
    pub cwd: String,
}

/// A 2D boolean grid representing the QR code modules.
/// `true` = dark module, `false` = light module.
pub struct QrGrid {
    pub modules: Vec<bool>,
    /// Width (and height) in modules — QR codes are square.
    pub size: usize,
}

impl QrGrid {
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

/// Generate a QR code grid from a session payload.
///
/// Uses medium (M) error correction, which gives a good balance of
/// scannability and density for a ~150-byte JSON payload.
pub fn generate(payload: &SessionPayload) -> anyhow::Result<QrGrid> {
    let json = serde_json::to_string(payload)?;
    let code = QrCode::with_error_correction_level(&json, qrcode::EcLevel::M)?;
    let size = code.width();

    let mut modules = Vec::with_capacity(size * size);
    // qrcode::QrCode uses (x, y) coordinates.
    // We flatten row-major (y-major): row y, column x.
    for y in 0..size {
        for x in 0..size {
            // QrCode implements Index<(usize, usize)> to return Color
            let dark = code[(x, y)] == qrcode::types::Color::Dark;
            modules.push(dark);
        }
    }

    Ok(QrGrid { modules, size })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_qr_for_session_payload() {
        let payload = SessionPayload {
            v: 1,
            sid: "abc123".into(),
            host: "mac-top.local".into(),
            port: 9876,
            cwd: "/home/user/project".into(),
        };
        let grid = generate(&payload).unwrap();
        assert!(grid.size >= 21); // at least version 1
        assert_eq!(grid.modules.len(), grid.size * grid.size);

        // Finder patterns in corners should be dark (top-left area).
        // Version ≥1 always has a 7×7 finder at (0,0).
        assert!(grid.is_dark(0, 0));
        assert!(grid.is_dark(6, 0));
        assert!(grid.is_dark(0, 6));
    }

    #[test]
    fn payload_roundtrips() {
        let payload = SessionPayload {
            v: 1,
            sid: uuid::Uuid::new_v4().to_string(),
            host: "mac-top.peterlodri-sec.ts.net".into(),
            port: 9876,
            cwd: "/Users/peter/workspace/entheai".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: SessionPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sid, payload.sid);
        assert_eq!(parsed.host, payload.host);
    }
}
