//! Image MIME-type inference: by file extension first, falling back to a
//! magic-byte sniff of the file's own header for extensionless/renamed files.
//! No `infer`-style crate — the format set entheai's vision backends actually
//! accept is small and stable enough to hand-list.

use std::path::Path;

/// MIME type from a path's extension, case-insensitively.
fn mime_from_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        _ => return None,
    })
}

/// MIME type from a file's magic-byte header. Covers only formats whose
/// signature is a fixed, cheap byte match — HEIC/HEIF's `ftyp` box parsing
/// isn't worth it here since those files reach this path only via extension.
pub(crate) fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

/// MIME type for a path-based image: extension first (cheap, handles renamed
/// files with a truthful extension), else sniff the bytes actually read.
pub(crate) fn mime_type_for(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    mime_from_extension(path).or_else(|| sniff_mime(bytes))
}

/// A plausible file extension for a MIME type, for spilling pasted bytes to a
/// temp file the `agy` CLI can read (it wants a real path, not raw bytes).
/// Unknown MIME types get a generic `.img` — the CLI still gets valid image
/// bytes, just without a format hint from the filename.
pub(crate) fn extension_for_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/heic" => "heic",
        "image/heif" => "heif",
        _ => "img",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn extension_wins_over_sniff_when_present() {
        let png_bytes = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        assert_eq!(
            mime_type_for(&PathBuf::from("shot.jpg"), &png_bytes),
            Some("image/jpeg")
        );
    }

    #[test]
    fn falls_back_to_sniffing_when_extension_is_missing() {
        let png_bytes = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        assert_eq!(
            mime_type_for(&PathBuf::from("pasted"), &png_bytes),
            Some("image/png")
        );
    }

    #[test]
    fn sniffs_jpeg_gif_webp_bmp() {
        assert_eq!(
            mime_type_for(&PathBuf::from("x"), &[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_type_for(&PathBuf::from("x"), b"GIF89a...."),
            Some("image/gif")
        );
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(
            mime_type_for(&PathBuf::from("x"), &webp),
            Some("image/webp")
        );
        assert_eq!(
            mime_type_for(&PathBuf::from("x"), b"BMxxxxxxxx"),
            Some("image/bmp")
        );
    }

    #[test]
    fn unrecognized_bytes_and_extension_yield_none() {
        assert_eq!(mime_type_for(&PathBuf::from("x.txt"), b"hello"), None);
    }

    #[test]
    fn extension_for_mime_round_trips_known_types_and_falls_back() {
        assert_eq!(extension_for_mime("image/png"), "png");
        assert_eq!(extension_for_mime("image/jpeg"), "jpg");
        assert_eq!(extension_for_mime("application/octet-stream"), "img");
    }
}
