//! ayeOS ternary matrix loading + strict validation.
//!
//! Every `mNNN.json` file follows the verified schema: `name`, `dim` (output
//! rows N), `in_features` (input cols K), `group_size` (always 64), `codes`
//! (N × K/16 packed `u32`), `scales` (N × K/64), `seed_hash`. `index.json`
//! carries capsule metadata plus the 168-matrix manifest.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::codes::{self, word_has_illegal_code};

/// A single loaded ayeOS ternary matrix.
///
/// `codes` packs the `dim × in_features` ternary codes as `u32` words
/// (16 codes per word, LSB-first, row-major over `in_features`); `scales`
/// holds one `f32` per `group_size` chunk of each row.
#[derive(Debug, Clone)]
pub struct AyeosMatrix {
    pub name: String,
    pub dim: usize,
    pub in_features: usize,
    pub group_size: usize,
    pub codes: Vec<u32>,
    pub scales: Vec<f32>,
}

impl AyeosMatrix {
    /// Packed `u32` words per row (`in_features / 16`).
    pub fn words_per_row(&self) -> usize {
        self.in_features / codes::CODES_PER_WORD
    }

    /// Scale groups per row (`in_features / group_size`).
    pub fn groups_per_row(&self) -> usize {
        self.in_features / self.group_size
    }

    /// Total parameter count (`dim × in_features`).
    pub fn param_count(&self) -> usize {
        self.dim * self.in_features
    }
}

/// `index.json` — capsule metadata + the matrix manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct AyeosIndex {
    pub capsule_id: Option<String>,
    pub metadata: Option<AyeosIndexMetadata>,
    pub matrices: Vec<AyeosIndexEntry>,
}

/// `index.json` metadata block.
#[derive(Debug, Clone, Deserialize)]
pub struct AyeosIndexMetadata {
    pub base_model: Option<String>,
    pub group_size: Option<usize>,
    pub checkpoint_sha256: Option<String>,
}

/// One `matrices` manifest entry.
#[derive(Debug, Clone, Deserialize)]
pub struct AyeosIndexEntry {
    pub file: String,
    pub name: String,
    pub dim: usize,
    pub in_features: usize,
    pub group_size: usize,
}

/// Loading / validation failure.
#[derive(Debug)]
pub enum LoadError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    BadGroupSize {
        group_size: usize,
    },
    InFeaturesNotMultiple {
        in_features: usize,
        group_size: usize,
    },
    CodeCountMismatch {
        actual: usize,
        expected: usize,
    },
    ScaleCountMismatch {
        actual: usize,
        expected: usize,
    },
    IllegalCode {
        file: PathBuf,
        word: usize,
        field: usize,
    },
    ManifestMismatch {
        file: String,
        what: &'static str,
        manifest: String,
        actual: String,
    },
    DimOverflow {
        dim: usize,
        in_features: usize,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Json { path, source } => write!(f, "cannot parse {}: {source}", path.display()),
            Self::BadGroupSize { group_size } => {
                write!(f, "group_size {group_size} != 64 (ayeOS schema)")
            }
            Self::InFeaturesNotMultiple {
                in_features,
                group_size,
            } => {
                write!(
                    f,
                    "in_features {in_features} not a multiple of group_size {group_size}"
                )
            }
            Self::CodeCountMismatch { actual, expected } => {
                write!(f, "codes length {actual} != expected N×K/16 = {expected}")
            }
            Self::ScaleCountMismatch { actual, expected } => {
                write!(f, "scales length {actual} != expected N×K/64 = {expected}")
            }
            Self::IllegalCode { file, word, field } => {
                write!(
                    f,
                    "illegal code > 2 in {} word {word} field {field}",
                    file.display()
                )
            }
            Self::ManifestMismatch {
                file,
                what,
                manifest,
                actual,
            } => {
                write!(
                    f,
                    "manifest {what} mismatch for {file}: manifest {manifest} != actual {actual}"
                )
            }
            Self::DimOverflow { dim, in_features } => {
                write!(f, "dim {dim} × in_features {in_features} overflows usize")
            }
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Raw JSON shape of a `mNNN.json` file (unknown keys — incl. `seed_hash` —
/// are ignored by serde; only the fields the loader validates are read).
#[derive(Debug, Deserialize)]
struct RawMatrix {
    name: String,
    dim: usize,
    in_features: usize,
    group_size: usize,
    codes: Vec<u32>,
    scales: Vec<f32>,
}

/// Load and strictly validate a single `mNNN.json` matrix file.
pub fn load_file(path: impl AsRef<Path>) -> Result<AyeosMatrix, LoadError> {
    let path = path.as_ref();
    let text = fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let raw: RawMatrix = serde_json::from_str(&text).map_err(|source| LoadError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    validate(raw, path)
}

fn validate(raw: RawMatrix, path: &Path) -> Result<AyeosMatrix, LoadError> {
    let RawMatrix {
        name,
        dim,
        in_features,
        group_size,
        codes,
        scales,
    } = raw;

    if group_size != 64 {
        return Err(LoadError::BadGroupSize { group_size });
    }
    if in_features % group_size != 0 {
        return Err(LoadError::InFeaturesNotMultiple {
            in_features,
            group_size,
        });
    }
    let dim_x_in = dim
        .checked_mul(in_features)
        .ok_or(LoadError::DimOverflow { dim, in_features })?;
    let expected_codes = dim_x_in / codes::CODES_PER_WORD;
    if codes.len() != expected_codes {
        return Err(LoadError::CodeCountMismatch {
            actual: codes.len(),
            expected: expected_codes,
        });
    }
    let expected_scales = dim_x_in / group_size;
    if scales.len() != expected_scales {
        return Err(LoadError::ScaleCountMismatch {
            actual: scales.len(),
            expected: expected_scales,
        });
    }
    // No code > 2: fast whole-word scan, then locate the exact field on failure.
    if let Some(word) = codes.iter().position(|w| word_has_illegal_code(*w)) {
        let field = find_illegal_field(codes[word]);
        return Err(LoadError::IllegalCode {
            file: path.to_path_buf(),
            word,
            field,
        });
    }
    Ok(AyeosMatrix {
        name,
        dim,
        in_features,
        group_size,
        codes,
        scales,
    })
}

/// Locate the exact 2-bit field holding code 3 (for error reporting).
fn find_illegal_field(word: u32) -> usize {
    for i in 0..codes::CODES_PER_WORD {
        if ((word >> (codes::BITS_PER_CODE * i as u32)) & 0b11) == 0b11 {
            return i;
        }
    }
    unreachable!("word_has_illegal_code returned true but no field is 3")
}

/// Load and parse `index.json`.
pub fn load_index(path: impl AsRef<Path>) -> Result<AyeosIndex, LoadError> {
    let path = path.as_ref();
    let text = fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| LoadError::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// Load every matrix in a directory, strictly validated.
///
/// Prefers the `index.json` manifest (each entry is cross-checked against the
/// loaded matrix); falls back to sorted `m*.json` globbing if the index is
/// absent. Every file must satisfy the ayeOS schema.
pub fn load_dir(path: impl AsRef<Path>) -> Result<Vec<AyeosMatrix>, LoadError> {
    let dir = path.as_ref();
    let index_path = dir.join("index.json");

    let mut matrices = Vec::new();
    if index_path.exists() {
        let index = load_index(&index_path)?;
        for entry in &index.matrices {
            let m = load_file(dir.join(&entry.file))?;
            check_manifest(entry, &m)?;
            matrices.push(m);
        }
    } else {
        for file in sorted_matrix_files(dir)? {
            matrices.push(load_file(dir.join(file))?);
        }
    }
    Ok(matrices)
}

fn check_manifest(entry: &AyeosIndexEntry, m: &AyeosMatrix) -> Result<(), LoadError> {
    if m.name != entry.name {
        return Err(LoadError::ManifestMismatch {
            file: entry.file.clone(),
            what: "name",
            manifest: entry.name.clone(),
            actual: m.name.clone(),
        });
    }
    if m.dim != entry.dim {
        return Err(LoadError::ManifestMismatch {
            file: entry.file.clone(),
            what: "dim",
            manifest: entry.dim.to_string(),
            actual: m.dim.to_string(),
        });
    }
    if m.in_features != entry.in_features {
        return Err(LoadError::ManifestMismatch {
            file: entry.file.clone(),
            what: "in_features",
            manifest: entry.in_features.to_string(),
            actual: m.in_features.to_string(),
        });
    }
    if m.group_size != entry.group_size {
        return Err(LoadError::ManifestMismatch {
            file: entry.file.clone(),
            what: "group_size",
            manifest: entry.group_size.to_string(),
            actual: m.group_size.to_string(),
        });
    }
    Ok(())
}

/// `m*.json` files in `dir`, sorted by name (zero-padded → numeric order).
fn sorted_matrix_files(dir: &Path) -> Result<Vec<String>, LoadError> {
    let mut files: Vec<String> = fs::read_dir(dir)
        .map_err(|source| LoadError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('m') && n.ends_with(".json"))
        .collect();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Real ayeos data dir: `AYEOS_DATA_DIR` env override, else relative to the
    /// crate root (tests run with CWD = package root).
    fn data_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("AYEOS_DATA_DIR") {
            return PathBuf::from(dir);
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../pocoo.vaked.dev/demos/quantal")
    }

    fn load_all() -> Vec<AyeosMatrix> {
        let dir = data_dir();
        let matrices = load_dir(&dir).unwrap_or_else(|e| {
            panic!(
                "load_dir({}) failed: {e} — set AYEOS_DATA_DIR to the quantal data dir",
                dir.display()
            )
        });
        assert_eq!(matrices.len(), 168, "expected 168 ayeOS matrices");
        matrices
    }

    /// Code-occurrence counts per 8-bit chunk (4 two-bit fields per byte).
    const fn byte_counts(b: u8) -> (u8, u8, u8, u8) {
        let mut c0 = 0u8;
        let mut c1 = 0u8;
        let mut c2 = 0u8;
        let mut c3 = 0u8;
        let mut i = 0u8;
        while i < 4 {
            match (b >> (2 * i)) & 0b11 {
                0 => c0 += 1,
                1 => c1 += 1,
                2 => c2 += 1,
                _ => c3 += 1,
            }
            i += 1;
        }
        (c0, c1, c2, c3)
    }

    const CODE_COUNTS: [(u8, u8, u8, u8); 256] = {
        let mut table = [(0u8, 0u8, 0u8, 0u8); 256];
        let mut i = 0usize;
        while i < 256 {
            table[i] = byte_counts(i as u8);
            i += 1;
        }
        table
    };

    #[test]
    fn validate_rejects_dim_times_in_features_overflow_instead_of_wrapping() {
        let raw = RawMatrix {
            name: "bogus".to_string(),
            dim: usize::MAX,
            in_features: 64,
            group_size: 64,
            codes: Vec::new(),
            scales: Vec::new(),
        };
        let err = validate(raw, Path::new("bogus.json")).unwrap_err();
        assert!(
            matches!(err, LoadError::DimOverflow { dim, in_features } if dim == usize::MAX && in_features == 64),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn global_invariants_over_all_168_matrices() {
        let matrices = load_all();

        // index.json metadata sanity.
        let index = load_index(data_dir().join("index.json")).expect("index.json loads");
        assert_eq!(index.matrices.len(), 168);
        let meta = index.metadata.as_ref().expect("metadata present");
        assert_eq!(meta.base_model.as_deref(), Some("Qwen/Qwen2.5-0.5B"));
        assert_eq!(meta.group_size, Some(64));
        assert!(meta
            .checkpoint_sha256
            .as_deref()
            .is_some_and(|s| s.len() == 64));

        // Every code ≤ 2, every scale finite, sign balance ~50/50.
        let mut total_params = 0usize;
        let mut plus = 0u64;
        let mut minus = 0u64;
        for m in &matrices {
            total_params += m.param_count();
            assert_eq!(m.group_size, 64, "group_size != 64 in {}", m.name);
            for w in &m.codes {
                assert!(!word_has_illegal_code(*w), "code > 2 in {}", m.name);
            }
            for s in &m.scales {
                assert!(s.is_finite(), "non-finite scale in {}", m.name);
            }
            for w in m.codes.iter().copied() {
                let (c0, _c1, c2, c3) = CODE_COUNTS[(w & 0xFF) as usize];
                let (c0b, _c1b, c2b, c3b) = CODE_COUNTS[((w >> 8) & 0xFF) as usize];
                let (c0c, _c1c, c2c, c3c) = CODE_COUNTS[((w >> 16) & 0xFF) as usize];
                let (c0d, _c1d, c2d, c3d) = CODE_COUNTS[((w >> 24) & 0xFF) as usize];
                minus += (c0 + c0b + c0c + c0d) as u64;
                plus += (c2 + c2b + c2c + c2d) as u64;
                assert_eq!(c3 + c3b + c3c + c3d, 0, "code 3 in {}", m.name);
            }
        }

        // Total parameter count pins the schema interpretation.
        assert_eq!(
            total_params, 357_826_560,
            "Σ dim×in_features over 168 matrices"
        );

        // Sign balance: fraction of +1 among non-zero codes is ~50%.
        let nonzero = plus + minus;
        let frac_plus = plus as f64 / nonzero as f64;
        assert!(
            (0.45..=0.55).contains(&frac_plus),
            "plus fraction {frac_plus} out of 50/50 balance ({plus} vs {minus})"
        );
    }
}
