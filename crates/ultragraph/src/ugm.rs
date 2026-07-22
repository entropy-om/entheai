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
        let _ = bytes;
        todo!("agy port: parse header/trees/ultra-edges/weights per the layout above")
    }

    /// Index of the sink tree (highest-index tree with no outgoing ultra-edge).
    pub fn sink_idx(&self) -> usize {
        todo!("agy port: last i not present as any edge.src_idx")
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
        let _ = x;
        todo!("agy port: topo interpreter + dense _forward_tree; see SPEC.md §run")
    }
}
