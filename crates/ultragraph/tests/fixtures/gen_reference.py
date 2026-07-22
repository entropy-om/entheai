#!/usr/bin/env python3
"""Generate the language-independent conformance fixture for the Rust port of
ultra-graph's deployed-inference core.

Run against the ultra-graph *source tree* (no install needed):

    PYTHONPATH=/path/to/ultra-graph python3 gen_reference.py

Emits, next to this script:
  - model.ugm      — a tiny deterministic .ugm binary (v1 format)
  - reference.json — the input, the run() output, and quant/pack/tokenize vectors

The Rust `crates/ultragraph` port MUST reproduce every value here from the same
model.ugm — this file is the acceptance test's ground truth. Weights are hand-set
(no RNG) so the fixture is byte-stable across regenerations.
"""

import json
import os

import numpy as np

from ultragraph.pack import pack_ternary, unpack_ternary
from ultragraph.quant import dequant, quantize_act_int8, quantize_weight_ternary
from ultragraph.tokenize import ByteTokenizer
from ultragraph.ugm import UGMFile, UGMTree, UGMUltraEdge, load_ugm, save_ugm

HERE = os.path.dirname(os.path.abspath(__file__))


def build_model() -> UGMFile:
    # tree0: dense 4->4, relu. tree1: dense 4->4, identity(none).
    # Wiring: x -> tree0 (relu); tree1 takes tree0's output (plain) and also adds
    # it back as a residual. sink = tree1. Exercises relu, none, plain + residual.
    w0 = np.array(
        [[1, -1, 0, 1], [0, 1, -1, 0], [-1, 0, 1, 1], [1, 1, -1, 0]], dtype=np.int8
    )
    b0 = np.array([0.5, -0.25, 0.0, 1.0], dtype=np.float32)
    w1 = np.array(
        [[0, 1, 1, -1], [1, 0, -1, 0], [-1, -1, 0, 1], [0, 1, 0, 1]], dtype=np.int8
    )
    b1 = np.array([0.1, 0.2, -0.3, 0.4], dtype=np.float32)

    t0 = UGMTree(kind=0, act=1, in_dim=4, out_dim=4, name="t0", w_scale=0.7, wq=w0, bias=b0)
    t1 = UGMTree(kind=0, act=0, in_dim=4, out_dim=4, name="t1", w_scale=1.3, wq=w1, bias=b1)
    edges = [
        UGMUltraEdge(src_idx=0, dst_idx=1, kind=0),  # plain: tree0 -> tree1 input
        UGMUltraEdge(src_idx=0, dst_idx=1, kind=1),  # residual: add tree0 output
    ]
    m = UGMFile(trees=[t0, t1], ultra_edges=edges)
    m.header.n_trees = 2
    m.header.n_ultra_edges = 2
    return m


def main() -> None:
    m = build_model()
    ugm_path = os.path.join(HERE, "model.ugm")
    save_ugm(ugm_path, m, packed=False)
    # round-trip through the on-disk format so run() uses exactly what Rust loads
    loaded = load_ugm(ugm_path)

    x = np.array([[0.5, -1.0, 2.0, 0.25], [1.0, 1.0, -1.0, 0.0]], dtype=np.float32)
    y = loaded.run(x)

    # quant / pack / tokenize reference vectors
    wq_in = np.array([0.9, -0.2, 0.05, -1.4, 0.6], dtype=np.float32)
    q_w, s_w = quantize_weight_ternary(wq_in)
    act_in = np.array([3.0, -7.0, 0.5, 1.2], dtype=np.float32)
    q_a, s_a = quantize_act_int8(act_in)
    tern = np.array([-1, 0, 1, 1, -1, 0, 1], dtype=np.int8)  # len 7 -> pad to 10
    packed = pack_ternary(tern)
    unpacked = unpack_ternary(packed, len(tern))

    ref = {
        "note": "conformance fixture for the Rust ultragraph port; regenerate with gen_reference.py",
        "input": x.tolist(),
        "output": y.tolist(),
        "quant_weight": {"input": wq_in.tolist(), "q": q_w.astype(int).tolist(), "scale": s_w},
        "quant_act": {"input": act_in.tolist(), "q": q_a.astype(int).tolist(), "scale": s_a},
        "pack": {"ternary": tern.astype(int).tolist(),
                 "packed": packed.astype(int).tolist(),
                 "unpacked": unpacked.astype(int).tolist()},
        "tokenize": {"text": "héllo", "ids": ByteTokenizer().encode("héllo").tolist()},
        "dequant_check": dequant(q_w, s_w).tolist(),
    }
    with open(os.path.join(HERE, "reference.json"), "w") as fh:
        json.dump(ref, fh, indent=2)
    print(f"wrote {ugm_path} ({os.path.getsize(ugm_path)} bytes) + reference.json")
    print("output[0] =", y[0].tolist())


if __name__ == "__main__":
    main()
