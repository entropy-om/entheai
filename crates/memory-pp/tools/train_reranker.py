#!/usr/bin/env python3
"""Train a 1-bit (ternary) UGM reranker using ultra-graph and export to .ugm.

This script generates synthetic topical query-document triples, featurizes them
into 768-dim feature vectors matching entheai's Rust mesh featurizer byte-for-byte,
trains a single dense ternary linear reranker with straight-through estimator (STE)
and margin ranking loss, quantizes weights to BitNet ternary {-1, 0, +1}, and exports
the model to crates/memory-pp/models/reranker.ugm.
"""

import sys
import random
from pathlib import Path
import numpy as np

# Ensure ultra-graph is on python path
SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent.parent.parent  # entheai repo root
ULTRAGRAPH_PATH = Path("/Users/peter.lodri/workspace/peterlodri-sec/ultra-graph")

if str(ULTRAGRAPH_PATH) not in sys.path:
    sys.path.insert(0, str(ULTRAGRAPH_PATH))

from ultragraph.core import Tree
from ultragraph.autograd import Tensor
from ultragraph.optim import SGD
from ultragraph.quant import quantize_weight_ternary
from ultragraph.ugm import UGMFile, UGMTree, KIND_DENSE, ACT_NONE, save_ugm, load_ugm


def byte_histogram(s: str) -> np.ndarray:
    """256-length f32 normalized byte histogram (0 for empty string)."""
    h = np.zeros(256, dtype=np.float32)
    b = s.encode("utf-8")
    if len(b) == 0:
        return h
    counts = np.bincount(np.frombuffer(b, dtype=np.uint8), minlength=256)
    return (counts[:256] / float(len(b))).astype(np.float32)


def featurize(query: str, text: str) -> np.ndarray:
    """768-dim feature vector: [query_hist(256) | text_hist(256) | query⊙text(256)]."""
    q = byte_histogram(query)
    t = byte_histogram(text)
    inter = q * t
    res = np.concatenate([q, t, inter], axis=0).astype(np.float32)
    assert res.shape == (768,), f"Expected shape (768,), got {res.shape}"
    return res


# Topical dataset definition
TOPICS = {
    "auth": [
        "authentication", "login", "token", "session", "credential", "oauth",
        "jwt", "password", "sso", "bearer", "refresh", "permission", "identity"
    ],
    "disk": [
        "usage", "inode", "mount", "filesystem", "partition", "storage",
        "volume", "sda1", "nvme", "directory", "path", "quota", "capacity"
    ],
    "render": [
        "rendering", "pixel", "shader", "frame", "texture", "raster",
        "canvas", "viewport", "animation", "draw", "vulkan", "buffer", "graphics"
    ],
    "net": [
        "network", "socket", "port", "packet", "router", "bandwidth",
        "latency", "interface", "tcp", "udp", "dns", "gateway", "connection"
    ],
    "db": [
        "database", "query", "sql", "index", "table", "schema",
        "record", "transaction", "migration", "postgres", "sqlite", "primary", "foreign"
    ],
    "memory": [
        "memory", "cache", "heap", "stack", "pointer", "allocation",
        "buffer", "garbage", "leak", "malloc", "dealloc", "lru", "segment"
    ],
    "proc": [
        "process", "thread", "spawn", "signal", "mutex", "lock",
        "deadlock", "schedule", "worker", "async", "coroutine", "exec", "task"
    ],
    "compiler": [
        "compiler", "syntax", "parse", "ast", "lexer", "symbol",
        "scope", "optimize", "codegen", "grammar", "token", "parser", "abstract"
    ]
}

TEMPLATES = [
    "the {0} {1} {2} flow and system operation",
    "handling {0} {1} for user {2} details",
    "monitoring {0} {1} status on {2} cluster",
    "{0} {1} report: 42% full on {2}",
    "executing {0} {1} pipeline for {2} process",
    "configuring {0} {1} settings for {2} node",
    "{0} {1} {2} verification trace log"
]


def generate_triple(rng: random.Random):
    """Generate a (query, relevant, irrelevant) triple."""
    t_name = rng.choice(list(TOPICS.keys()))
    words = TOPICS[t_name]

    # Query words (2-3 words from topic)
    q_words = rng.sample(words, 3)
    query = " ".join(q_words)

    # Relevant document (mix of query words and extra topic words)
    all_r = list(dict.fromkeys(q_words + rng.sample(words, 1)))
    rng.shuffle(all_r)
    tmpl = rng.choice(TEMPLATES)
    rel = tmpl.format(all_r[0], all_r[1], all_r[2])

    # Irrelevant document (words from a different topic)
    other_t = rng.choice([k for k in TOPICS.keys() if k != t_name])
    irr_words = rng.sample(TOPICS[other_t], 3)
    irr_tmpl = rng.choice(TEMPLATES)
    irr = irr_tmpl.format(irr_words[0], irr_words[1], irr_words[2])

    return query, rel, irr


def main():
    # Set seeds for reproducibility
    seed = 0
    rng = random.Random(seed)
    np.random.seed(seed)

    print("Generating synthetic topical dataset...")
    n_train = 5000
    n_test = 1000
    train_triples = [generate_triple(rng) for _ in range(n_train)]
    test_triples = [generate_triple(rng) for _ in range(n_test)]

    # Featurize dataset
    X_rel = np.array([featurize(q, rel) for q, rel, _ in train_triples], dtype=np.float32)
    X_irr = np.array([featurize(q, irr) for q, _, irr in train_triples], dtype=np.float32)

    # Build dense ternary linear tree (in_dim=768, out_dim=1, act="none")
    tree = Tree.dense(768, 1, name="rerank", act="none")
    
    # Initialize interaction block (512..767) positively, rest 0
    w_init = np.zeros((1, 768), dtype=np.float32)
    w_init[0, 512:] = 0.5
    tree.adhoc["w_master"].data[:] = w_init
    tree.requantize()

    opt = SGD(tree, lr=2.0)
    batch_size = 64
    epochs = 60
    num_samples = len(train_triples)

    print(f"Training reranker model ({epochs} epochs, batch_size={batch_size}, samples={num_samples})...")
    for epoch in range(1, epochs + 1):
        indices = np.arange(num_samples)
        np.random.shuffle(indices)
        total_loss = 0.0

        for start_idx in range(0, num_samples, batch_size):
            batch_idx = indices[start_idx:start_idx + batch_size]
            b_rel = Tensor(X_rel[batch_idx])
            b_irr = Tensor(X_irr[batch_idx])

            s_rel = tree.forward(b_rel)
            s_irr = tree.forward(b_irr)

            diff = s_rel - s_irr
            margin_tensor = Tensor(np.full_like(diff.data, 0.5))
            loss = (margin_tensor - diff).relu().mean()

            opt.zero_grad()
            loss.backward()

            # Zero out gradients for q (0..255) and t (256..511) so focus is on interaction co-occurrence
            tree.adhoc["w_master"].grad[0, :512] = 0.0

            opt.step()

            total_loss += float(loss.data) * len(batch_idx)

        if epoch % 20 == 0 or epoch == epochs:
            print(f"Epoch {epoch:2d}/{epochs} - Loss: {total_loss / num_samples:.4f}")

    # Quantize to deployed ternary weights
    w_master = tree.adhoc["w_master"].data
    bias = tree.adhoc["bias"].data
    wq, w_scale = quantize_weight_ternary(w_master)

    # Output paths
    output_dir = SCRIPT_DIR.parent / "models"
    output_dir.mkdir(parents=True, exist_ok=True)
    output_ugm_path = output_dir / "reranker.ugm"

    ugt = UGMTree(
        kind=KIND_DENSE,
        act=ACT_NONE,
        in_dim=768,
        out_dim=1,
        name="rerank",
        w_scale=float(w_scale),
        wq=wq,
        bias=bias,
    )
    ugm_file = UGMFile(trees=[ugt], ultra_edges=[])
    save_ugm(output_ugm_path, ugm_file)
    print(f"Exported trained .ugm model to {output_ugm_path}")

    # Evaluate DEPLOYED forward with load_ugm
    loaded_m = load_ugm(output_ugm_path)

    correct = 0
    for q, rel, irr in test_triples:
        f_rel = featurize(q, rel)
        f_irr = featurize(q, irr)
        s_rel = float(loaded_m.run([f_rel])[0][0])
        s_irr = float(loaded_m.run([f_irr])[0][0])
        if s_rel > s_irr:
            correct += 1

    acc = correct / len(test_triples)

    # Evaluate exact required test example
    exact_q = "auth login token"
    exact_rel = "the authentication login token refresh flow"
    exact_irr = "disk usage report: 42% full on /dev/sda1"
    exact_s_rel = float(loaded_m.run([featurize(exact_q, exact_rel)])[0][0])
    exact_s_irr = float(loaded_m.run([featurize(exact_q, exact_irr)])[0][0])

    print("\n--- EVALUATION SUMMARY ---")
    print(f"Dataset size: {n_train} train triples, {n_test} held-out test triples")
    print(f"Epochs: {epochs}")
    print(f"Held-out Accuracy: {acc * 100:.2f}% ({correct}/{n_test})")
    print(f"Exact Example Scores:")
    print(f"  Query: '{exact_q}'")
    print(f"  Relevant: '{exact_rel}' -> Score: {exact_s_rel:.4f}")
    print(f"  Irrelevant: '{exact_irr}' -> Score: {exact_s_irr:.4f}")
    print(f"  Correctly ranked exact example: {exact_s_rel > exact_s_irr}")

    assert acc >= 0.90, f"Held-out accuracy {acc*100:.2f}% is below 90% threshold!"
    assert exact_s_rel > exact_s_irr, "Exact test example was misclassified!"


if __name__ == "__main__":
    main()
