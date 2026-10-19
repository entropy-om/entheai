//! `.ugm` v1 — Ultragraph Module binary format + forward interpreter.
//! Port of `ultragraph/ugm.py` (the `load_ugm` reader + `UGMFile.run`).
//!
//! Binary layout (little-endian), spec v1:
//!
//! ```text
//! Header (32 bytes):
//!   0   4  magic          b"\x55\x47\x4D\x01" (UGM1)
//!   4   2  version        u16 (== 1)
//!   6   2  flags          u16 (bit0: packed weights)
//!   8   4  n_trees        u32
//!   12  4  n_ultra_edges  u32
//!   16  4  trees_offset   u32
//!   20  4  ue_offset      u32
//!   24  4  weights_offset u32
//!   28  4  weights_size   u32
//! Tree table (per tree, variable):
//!   kind:u8, act:u8, in_dim:u32, out_dim:u32, name_len:u16, name:[u8;name_len], w_scale:f32
//! Ultra-edge table (9 bytes each): src_idx:u32, dst_idx:u32, kind:u8
//! Weight data (per DENSE tree, in tree order): wq:[i8; out_dim*in_dim], bias:[f32; out_dim]
//! Optional trailing segments: seg_type:u32, seg_len:u32, seg_data (1=history, 2=metadata) — ignored here.
//! ```
//!
//! Activations: 0 none · 1 relu · 2 identity · 3 sigmoid · 4 tanh.
//! Ultra-edge kinds: 0 plain (feeds input) · 1 residual (adds src output).

/// `b"UGM1"`.
pub const MAGIC: [u8; 4] = [0x55, 0x47, 0x4D, 0x01];
pub const VERSION: u16 = 1;

pub const KIND_DENSE: u8 = 0;
pub const KIND_SPARSE: u8 = 1;

pub const ACT_NONE: u8 = 0;
pub const ACT_RELU: u8 = 1;
pub const ACT_IDENTITY: u8 = 2;
pub const ACT_SIGMOID: u8 = 3;
pub const ACT_TANH: u8 = 4;

pub const UE_PLAIN: u8 = 0;
pub const UE_RESIDUAL: u8 = 1;

/// A tree (a dense Linear block) in the `.ugm`.
#[derive(Debug, Clone)]
pub struct UgmTree {
    pub kind: u8,
    pub act: u8,
    pub in_dim: u32,
    pub out_dim: u32,
    pub name: String,
    pub w_scale: f32,
    /// int8 ternary weights, row-major `[out_dim][in_dim]` (dense only).
    pub wq: Option<Vec<i8>>,
    /// f32 bias `[out_dim]` (dense only).
    pub bias: Option<Vec<f32>>,
}

/// Typed wiring between trees.
#[derive(Debug, Clone, Copy)]
pub struct UgmUltraEdge {
    pub src_idx: u32,
    pub dst_idx: u32,
    pub kind: u8,
}

/// A complete `.ugm` module in memory.
#[derive(Debug, Clone)]
pub struct UgmFile {
    pub trees: Vec<UgmTree>,
    pub ultra_edges: Vec<UgmUltraEdge>,
}

impl UgmFile {
    /// Load and parse a `.ugm` file.
    pub fn load(path: &std::path::Path) -> std::io::Result<UgmFile> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Parse a `.ugm` module from its bytes. Rejects a bad magic / version with
    /// `io::ErrorKind::InvalidData`.
    pub fn from_bytes(bytes: &[u8]) -> std::io::Result<UgmFile> {
        if bytes.len() < 32 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "file shorter than 32-byte header",
            ));
        }

        if bytes[0..4] != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid UGM magic",
            ));
        }

        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if version != VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported UGM version {version}, expected {VERSION}"),
            ));
        }

        let flags = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        if flags & 1 != 0 {
            // Bit0 ("packed weights") is a real spec bit with no reader
            // support here (`unpack_ternary` exists but is never called) —
            // a packed file was silently parsed as raw i8 weights, giving
            // wrong values (or a spurious EOF once the shorter packed data
            // ran out). Refuse it outright rather than misinterpret it.
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "packed weights (flags bit0) are not supported by this reader",
            ));
        }
        let n_trees = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let n_ultra_edges = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let trees_offset = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
        let ue_offset = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
        let weights_offset = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
        let _weights_size = u32::from_le_bytes(bytes[28..32].try_into().unwrap()) as usize;

        // Parse tree table. Each tree entry is at least 16 bytes on disk
        // (kind, act, in_dim, out_dim, name_len — the name/scale tail is
        // validated per-entry below), so a file-supplied `n_trees` can never
        // legitimately exceed `bytes.len() / 16` — cap the allocation
        // request instead of trusting a corrupt/hostile count and aborting
        // on a multi-GB allocation failure.
        let n_trees = n_trees.min(bytes.len() / 16 + 1);
        let mut cursor = trees_offset;
        let mut trees = Vec::with_capacity(n_trees);
        for _ in 0..n_trees {
            if cursor + 16 > bytes.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unexpected EOF reading tree table",
                ));
            }
            let kind = bytes[cursor];
            let act = bytes[cursor + 1];
            cursor += 2;
            let in_dim = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
            let out_dim = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap());
            cursor += 8;
            let name_len =
                u16::from_le_bytes(bytes[cursor..cursor + 2].try_into().unwrap()) as usize;
            cursor += 2;
            if cursor + name_len + 4 > bytes.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unexpected EOF reading tree name and scale",
                ));
            }
            let name = std::str::from_utf8(&bytes[cursor..cursor + name_len])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
                .to_string();
            cursor += name_len;
            let w_scale = f32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
            cursor += 4;

            trees.push(UgmTree {
                kind,
                act,
                in_dim,
                out_dim,
                name,
                w_scale,
                wq: None,
                bias: None,
            });
        }

        // Parse ultra-edge table. 9 bytes/entry — same reasoning as n_trees.
        let n_ultra_edges = n_ultra_edges.min(bytes.len() / 9 + 1);
        cursor = ue_offset;
        let mut ultra_edges = Vec::with_capacity(n_ultra_edges);
        for _ in 0..n_ultra_edges {
            if cursor + 9 > bytes.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unexpected EOF reading ultra-edge table",
                ));
            }
            let src_idx = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
            let dst_idx = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap());
            let kind = bytes[cursor + 8];
            cursor += 9;
            ultra_edges.push(UgmUltraEdge {
                src_idx,
                dst_idx,
                kind,
            });
        }

        // Validate cross-references before `run()` ever sees this graph:
        // `run()`/`_forward_tree` panic!() on exactly these three
        // conditions (a dangling `src_idx`/`dst_idx` makes the topological
        // loop in `run()` never resolve that tree, misdiagnosed as "cycle
        // detected"; a non-`KIND_DENSE` tree has no forward implementation;
        // a dim mismatch across an edge indexes a row out of bounds) — a
        // malformed-but-structurally-parseable `.ugm` file used to be
        // accepted here and only panic later, at query time, inside
        // `NativeMesh::score`.
        for ue in &ultra_edges {
            let (src_i, dst_i) = (ue.src_idx as usize, ue.dst_idx as usize);
            if src_i >= trees.len() || dst_i >= trees.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "ultra-edge references out-of-range tree index \
                         (src={src_i}, dst={dst_i}, n_trees={})",
                        trees.len()
                    ),
                ));
            }
            let (src, dst) = (&trees[src_i], &trees[dst_i]);
            let expected = if ue.kind == UE_RESIDUAL {
                dst.out_dim
            } else {
                dst.in_dim
            };
            if src.out_dim != expected {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "ultra-edge dim mismatch: tree {src_i} out_dim={} feeds tree \
                         {dst_i}, which expects {expected}",
                        src.out_dim
                    ),
                ));
            }
        }
        if let Some(bad) = trees.iter().find(|t| t.kind != KIND_DENSE) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "tree kind={} ({:?}) has no forward implementation — only \
                     KIND_DENSE is supported",
                    bad.kind, bad.name
                ),
            ));
        }

        // Parse weight data per dense tree
        cursor = weights_offset;
        for tree in &mut trees {
            if tree.kind == KIND_DENSE {
                let in_dim = tree.in_dim as usize;
                let out_dim = tree.out_dim as usize;
                let overflow_err = || {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "tree dims overflow while computing weight data size",
                    )
                };
                let wq_len = out_dim.checked_mul(in_dim).ok_or_else(overflow_err)?;
                let bias_len = out_dim.checked_mul(4).ok_or_else(overflow_err)?;
                let end = cursor
                    .checked_add(wq_len)
                    .and_then(|c| c.checked_add(bias_len))
                    .ok_or_else(overflow_err)?;
                if end > bytes.len() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "unexpected EOF reading weight data",
                    ));
                }
                let wq: Vec<i8> = bytes[cursor..cursor + wq_len]
                    .iter()
                    .map(|&b| b as i8)
                    .collect();
                cursor += wq_len;

                let bias_slice = &bytes[cursor..cursor + bias_len];
                let mut bias = Vec::with_capacity(out_dim);
                for chunk in bias_slice.chunks_exact(4) {
                    bias.push(f32::from_le_bytes(chunk.try_into().unwrap()));
                }
                cursor += bias_len;

                tree.wq = Some(wq);
                tree.bias = Some(bias);
            }
        }

        Ok(UgmFile { trees, ultra_edges })
    }

    /// Index of the sink tree (highest-index tree with no outgoing ultra-edge).
    pub fn sink_idx(&self) -> usize {
        let src_set: std::collections::HashSet<u32> =
            self.ultra_edges.iter().map(|e| e.src_idx).collect();
        for i in (0..self.trees.len()).rev() {
            if !src_set.contains(&(i as u32)) {
                return i;
            }
        }
        if self.trees.is_empty() {
            0
        } else {
            self.trees.len() - 1
        }
    }

    /// Interpret the module as a forward pass. `x` is a batch of rows `[B][in_dim]`;
    /// returns the sink tree's output `[B][out_dim]`.
    ///
    /// Topological execution (see `ugm.py::run`): a tree runs once all its incoming
    /// srcs are ready; PLAIN incoming srcs are summed to form the input (no incoming
    /// → the module input `x`); after the tree runs, each RESIDUAL incoming src's
    /// output is added. Dense tree forward: `out = x @ (wq·w_scale)ᵀ + bias`, then
    /// the activation. Detect a cycle (no progress) rather than looping forever.
    pub fn run(&self, x: &[Vec<f32>]) -> Vec<Vec<f32>> {
        if self.trees.is_empty() || x.is_empty() {
            return Vec::new();
        }

        let mut outputs: std::collections::HashMap<usize, Vec<Vec<f32>>> =
            std::collections::HashMap::new();
        let mut remaining: Vec<usize> = (0..self.trees.len()).collect();
        let mut incoming: std::collections::HashMap<usize, Vec<UgmUltraEdge>> =
            std::collections::HashMap::new();

        for &ue in &self.ultra_edges {
            incoming.entry(ue.dst_idx as usize).or_default().push(ue);
        }

        while !remaining.is_empty() {
            let mut progressed = false;
            let mut still = Vec::new();

            for &ti in &remaining {
                let edges = incoming.get(&ti).cloned().unwrap_or_default();
                if !edges
                    .iter()
                    .all(|e| outputs.contains_key(&(e.src_idx as usize)))
                {
                    still.push(ti);
                    continue;
                }

                let tree = &self.trees[ti];
                let plain_srcs: Vec<usize> = edges
                    .iter()
                    .filter(|e| e.kind == UE_PLAIN)
                    .map(|e| e.src_idx as usize)
                    .collect();

                let inp = if !plain_srcs.is_empty() {
                    let mut input_acc = outputs[&plain_srcs[0]].clone();
                    for &s in &plain_srcs[1..] {
                        let src_out = &outputs[&s];
                        for b in 0..input_acc.len() {
                            for c in 0..input_acc[b].len() {
                                input_acc[b][c] += src_out[b][c];
                            }
                        }
                    }
                    input_acc
                } else {
                    x.to_vec()
                };

                let mut out = self._forward_tree(tree, &inp);

                for e in &edges {
                    if e.kind == UE_RESIDUAL {
                        let res_out = &outputs[&(e.src_idx as usize)];
                        for b in 0..out.len() {
                            for c in 0..out[b].len() {
                                out[b][c] += res_out[b][c];
                            }
                        }
                    }
                }

                outputs.insert(ti, out);
                progressed = true;
            }

            if !progressed {
                panic!("cycle detected in .ugm graph");
            }
            remaining = still;
        }

        let sink = self.sink_idx();
        outputs.remove(&sink).unwrap_or_default()
    }

    fn _forward_tree(&self, tree: &UgmTree, x: &[Vec<f32>]) -> Vec<Vec<f32>> {
        if tree.kind == KIND_DENSE {
            if let Some(wq) = tree.wq.as_ref() {
                let batch_size = x.len();
                let in_dim = tree.in_dim as usize;
                let out_dim = tree.out_dim as usize;
                let bias = tree.bias.as_ref();

                let mut out = vec![vec![0.0f32; out_dim]; batch_size];

                for b in 0..batch_size {
                    for i in 0..out_dim {
                        let mut sum = 0.0f32;
                        let w_row_offset = i * in_dim;
                        for j in 0..in_dim {
                            sum += x[b][j] * (wq[w_row_offset + j] as f32 * tree.w_scale);
                        }
                        if let Some(b_vec) = bias {
                            sum += b_vec[i];
                        }

                        let val = match tree.act {
                            ACT_RELU => sum.max(0.0),
                            ACT_SIGMOID => 1.0 / (1.0 + (-sum).exp()),
                            ACT_TANH => sum.tanh(),
                            _ => sum,
                        };
                        out[b][i] = val;
                    }
                }
                return out;
            }
        }
        panic!("Tree kind={} forward not implemented", tree.kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ugm_residual_run() {
        // Tree 0: 2 -> 2 identity tree with wq = [[1, 0], [0, 1]], w_scale = 1.0, bias = [0, 0]
        // Tree 1: 2 -> 2 identity tree with wq = [[1, 0], [0, 1]], w_scale = 1.0, bias = [0, 0]
        // Ultra-edges: 0 -> 1 PLAIN, 0 -> 1 RESIDUAL
        // Input: [[1.0, 2.0]]
        // Expected tree 0 output: [[1.0, 2.0]]
        // Expected tree 1 output: forward([[1.0, 2.0]]) + residual([[1.0, 2.0]]) = [[2.0, 4.0]]
        let tree0 = UgmTree {
            kind: KIND_DENSE,
            act: ACT_IDENTITY,
            in_dim: 2,
            out_dim: 2,
            name: "t0".to_string(),
            w_scale: 1.0,
            wq: Some(vec![1, 0, 0, 1]),
            bias: Some(vec![0.0, 0.0]),
        };
        let tree1 = UgmTree {
            kind: KIND_DENSE,
            act: ACT_IDENTITY,
            in_dim: 2,
            out_dim: 2,
            name: "t1".to_string(),
            w_scale: 1.0,
            wq: Some(vec![1, 0, 0, 1]),
            bias: Some(vec![0.0, 0.0]),
        };
        let ugm = UgmFile {
            trees: vec![tree0, tree1],
            ultra_edges: vec![
                UgmUltraEdge {
                    src_idx: 0,
                    dst_idx: 1,
                    kind: UE_PLAIN,
                },
                UgmUltraEdge {
                    src_idx: 0,
                    dst_idx: 1,
                    kind: UE_RESIDUAL,
                },
            ],
        };

        assert_eq!(ugm.sink_idx(), 1);
        let out = ugm.run(&[vec![1.0, 2.0]]);
        assert_eq!(out, vec![vec![2.0, 4.0]]);
    }

    /// Hand-encode a minimal `.ugm` v1 byte buffer for `from_bytes` tests —
    /// `trees` is `(kind, act, in_dim, out_dim, name, w_scale)`, `edges` is
    /// `(src_idx, dst_idx, kind)`, `weights` is `(wq, bias)` per `KIND_DENSE`
    /// tree, in tree order.
    fn build_ugm_bytes(
        flags: u16,
        n_trees_override: Option<u32>,
        trees: &[(u8, u8, u32, u32, &str, f32)],
        edges: &[(u32, u32, u8)],
        weights: &[(Vec<i8>, Vec<f32>)],
    ) -> Vec<u8> {
        let mut trees_buf = Vec::new();
        for (kind, act, in_dim, out_dim, name, w_scale) in trees {
            trees_buf.push(*kind);
            trees_buf.push(*act);
            trees_buf.extend_from_slice(&in_dim.to_le_bytes());
            trees_buf.extend_from_slice(&out_dim.to_le_bytes());
            trees_buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
            trees_buf.extend_from_slice(name.as_bytes());
            trees_buf.extend_from_slice(&w_scale.to_le_bytes());
        }
        let mut ue_buf = Vec::new();
        for (src, dst, kind) in edges {
            ue_buf.extend_from_slice(&src.to_le_bytes());
            ue_buf.extend_from_slice(&dst.to_le_bytes());
            ue_buf.push(*kind);
        }
        let mut w_buf = Vec::new();
        for (wq, bias) in weights {
            for &b in wq {
                w_buf.push(b as u8);
            }
            for &f in bias {
                w_buf.extend_from_slice(&f.to_le_bytes());
            }
        }

        let trees_offset = 32u32;
        let ue_offset = trees_offset + trees_buf.len() as u32;
        let weights_offset = ue_offset + ue_buf.len() as u32;
        let weights_size = w_buf.len() as u32;

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&n_trees_override.unwrap_or(trees.len() as u32).to_le_bytes());
        out.extend_from_slice(&(edges.len() as u32).to_le_bytes());
        out.extend_from_slice(&trees_offset.to_le_bytes());
        out.extend_from_slice(&ue_offset.to_le_bytes());
        out.extend_from_slice(&weights_offset.to_le_bytes());
        out.extend_from_slice(&weights_size.to_le_bytes());
        out.extend_from_slice(&trees_buf);
        out.extend_from_slice(&ue_buf);
        out.extend_from_slice(&w_buf);
        out
    }

    fn one_dense_tree() -> Vec<(u8, u8, u32, u32, &'static str, f32)> {
        vec![(KIND_DENSE, ACT_IDENTITY, 2, 2, "t0", 1.0)]
    }

    #[test]
    fn from_bytes_rejects_packed_weights_flag_instead_of_misreading_it() {
        let bytes = build_ugm_bytes(
            1, // bit0 set: packed weights
            None,
            &one_dense_tree(),
            &[],
            &[(vec![1, 0, 0, 1], vec![0.0, 0.0])],
        );
        let err = UgmFile::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("packed"));
    }

    #[test]
    fn from_bytes_rejects_a_dangling_ultra_edge_index() {
        let bytes = build_ugm_bytes(
            0,
            None,
            &one_dense_tree(),
            &[(0, 5, UE_PLAIN)], // dst=5 doesn't exist (only tree 0)
            &[(vec![1, 0, 0, 1], vec![0.0, 0.0])],
        );
        let err = UgmFile::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("out-of-range"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_bytes_rejects_a_dim_mismatched_ultra_edge() {
        let trees = vec![
            (KIND_DENSE, ACT_IDENTITY, 2, 2, "t0", 1.0), // out_dim=2
            (KIND_DENSE, ACT_IDENTITY, 3, 2, "t1", 1.0), // in_dim=3 — mismatch
        ];
        let bytes = build_ugm_bytes(
            0,
            None,
            &trees,
            &[(0, 1, UE_PLAIN)],
            &[
                (vec![1, 0, 0, 1], vec![0.0, 0.0]),
                (vec![1, 0, 0, 1, 0, 0], vec![0.0, 0.0]),
            ],
        );
        let err = UgmFile::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("dim mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_bytes_rejects_a_kind_sparse_tree() {
        let trees = vec![(KIND_SPARSE, ACT_IDENTITY, 2, 2, "t0", 1.0)];
        let bytes = build_ugm_bytes(0, None, &trees, &[], &[]);
        let err = UgmFile::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("KIND_DENSE"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_bytes_caps_a_hostile_tree_count_instead_of_aborting() {
        // The header claims u32::MAX trees; the actual file has none. Must
        // fail cleanly and promptly (EOF), not attempt to
        // `Vec::with_capacity(u32::MAX)`.
        let bytes = build_ugm_bytes(0, Some(u32::MAX), &[], &[], &[]);
        let err = UgmFile::from_bytes(&bytes).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn from_bytes_accepts_a_well_formed_file_end_to_end() {
        let trees = vec![
            (KIND_DENSE, ACT_IDENTITY, 2, 2, "t0", 1.0),
            (KIND_DENSE, ACT_IDENTITY, 2, 2, "t1", 1.0),
        ];
        let bytes = build_ugm_bytes(
            0,
            None,
            &trees,
            &[(0, 1, UE_PLAIN), (0, 1, UE_RESIDUAL)],
            &[
                (vec![1, 0, 0, 1], vec![0.0, 0.0]),
                (vec![1, 0, 0, 1], vec![0.0, 0.0]),
            ],
        );
        let ugm = UgmFile::from_bytes(&bytes).unwrap();
        assert_eq!(ugm.run(&[vec![1.0, 2.0]]), vec![vec![2.0, 4.0]]);
    }
}
