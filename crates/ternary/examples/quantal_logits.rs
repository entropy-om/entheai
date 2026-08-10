//! quantal_logits — golden-logits gate helper.
//!
//! Computes the final-token logits of the quantal ternary model for the two
//! fixed gate prompts (identical template + tokenizer as
//! `MLX-QUANT/scripts/quantal_golden_logits.py`) and writes them as JSON:
//!
//! ```text
//! cargo run -p ternary --example quantal_logits -- <model_dir> <out.json>
//! ```
//!
//! The output is compared against the MLX reference logits by
//! `quantal_compare_logits.py` (acceptance: agreement within ~1e-2).

use std::path::Path;

use ternary::model::TernaryModel;
use ternary::tokenizer::ChatTokenizer;

/// Fixed gate prompts — MUST stay in sync with quantal_golden_logits.py.
const GATE_PROMPTS: [(&str, &str); 2] = [
    (
        "You are a helpful assistant.",
        "What is the capital of France? Answer in one word.",
    ),
    (
        "You are Qwen, created by Alibaba Cloud. You are a helpful assistant.",
        "Explain the concept of recursion in one short paragraph.",
    ),
];

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let model_dir = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: quantal_logits <model_dir> [out.json]"))?;
    let out_path = args.next();

    let model = TernaryModel::load(&model_dir)?;
    let tokenizer = ChatTokenizer::load(&model_dir)?;

    let mut results = Vec::new();
    for (i, (system, user)) in GATE_PROMPTS.iter().enumerate() {
        let prompt = tokenizer.apply_chat_template(&[("system", *system), ("user", *user)]);
        let ids = tokenizer.encode(&prompt)?;
        let logits = model.logits_last(&ids)?;
        println!(
            "prompt {}: {} tokens (ids {:?}..), top-3: {:?}",
            i + 1,
            ids.len(),
            &ids[..ids.len().min(8)],
            top_k(&logits, 3)
        );
        results.push(serde_json::json!({
            "prompt": i + 1,
            "system": system,
            "user": user,
            "n_tokens": ids.len(),
            "logits": logits,
        }));
    }

    let doc = serde_json::json!({
        "model_dir": Path::new(&model_dir).canonicalize()?.to_string_lossy(),
        "logit_tolerance_note": "abs/rel ~1e-2",
        "prompts": results,
    });

    match out_path {
        Some(path) => {
            std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
            println!("wrote {path}");
        }
        None => println!("{}", serde_json::to_string(&doc)?),
    }
    Ok(())
}

fn top_k(logits: &[f32], k: usize) -> Vec<(u32, f32)> {
    let mut idx: Vec<usize> = (0..logits.len()).collect();
    idx.sort_by(|a, b| logits[*b].partial_cmp(&logits[*a]).unwrap());
    idx.into_iter()
        .take(k)
        .map(|i| (i as u32, logits[i]))
        .collect()
}
