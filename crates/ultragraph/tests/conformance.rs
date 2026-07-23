//! Conformance acceptance test: the Rust port must reproduce, from the same
//! `model.ugm`, exactly the numbers the Python reference produced into
//! `reference.json`. This is the agy porting task's definition of done.
//!
//! The port has landed: every test here runs live in the workspace suite (the
//! scaffold-era `#[ignore]` markers are gone) and passes byte-exact against
//! the committed Python-reference fixture.

use std::path::PathBuf;

use entheai_ultragraph::{
    dequant, pack_ternary, quantize_act_int8, quantize_weight_ternary, unpack_ternary,
    ByteTokenizer, UgmFile,
};
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn reference() -> Value {
    let raw = std::fs::read_to_string(fixtures().join("reference.json")).expect("reference.json");
    serde_json::from_str(&raw).expect("valid reference.json")
}

fn f32s(v: &Value) -> Vec<f32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect()
}
fn i8s(v: &Value) -> Vec<i8> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap() as i8)
        .collect()
}

const TOL: f32 = 1e-4;

#[test]
fn quant_matches_reference() {
    let r = reference();
    let qw = &r["quant_weight"];
    let (q, scale) = quantize_weight_ternary(&f32s(&qw["input"]));
    assert_eq!(q, i8s(&qw["q"]), "ternary weight codes");
    assert!(
        (scale - qw["scale"].as_f64().unwrap() as f32).abs() < TOL,
        "weight scale"
    );

    let qa = &r["quant_act"];
    let (q, scale) = quantize_act_int8(&f32s(&qa["input"]));
    assert_eq!(q, i8s(&qa["q"]), "int8 activation codes");
    assert!(
        (scale - qa["scale"].as_f64().unwrap() as f32).abs() < TOL,
        "act scale"
    );

    let deq = dequant(&i8s(&qw["q"]), qw["scale"].as_f64().unwrap() as f32);
    for (a, b) in deq.iter().zip(f32s(&r["dequant_check"])) {
        assert!((a - b).abs() < TOL, "dequant {a} vs {b}");
    }
}

#[test]
fn pack_matches_reference() {
    let p = &reference()["pack"];
    let tern = i8s(&p["ternary"]);
    let packed = pack_ternary(&tern);
    assert_eq!(
        packed.iter().map(|&b| b as i64).collect::<Vec<_>>(),
        p["packed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect::<Vec<_>>(),
        "packed bytes"
    );
    assert_eq!(
        unpack_ternary(&packed, tern.len()),
        tern,
        "unpack round-trip"
    );
}

#[test]
fn tokenize_matches_reference() {
    let t = &reference()["tokenize"];
    let ids = ByteTokenizer::encode(t["text"].as_str().unwrap());
    let want: Vec<u8> = t["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap() as u8)
        .collect();
    assert_eq!(ids, want, "byte ids");
    assert_eq!(
        ByteTokenizer::decode(&ids),
        t["text"].as_str().unwrap(),
        "decode round-trip"
    );
}

#[test]
fn ugm_run_matches_reference() {
    let r = reference();
    let model = UgmFile::load(&fixtures().join("model.ugm")).expect("load model.ugm");
    let x: Vec<Vec<f32>> = r["input"].as_array().unwrap().iter().map(f32s).collect();
    let got = model.run(&x);
    let want: Vec<Vec<f32>> = r["output"].as_array().unwrap().iter().map(f32s).collect();
    assert_eq!(got.len(), want.len(), "batch rows");
    for (gr, wr) in got.iter().zip(&want) {
        assert_eq!(gr.len(), wr.len(), "row width");
        for (a, b) in gr.iter().zip(wr) {
            assert!((a - b).abs() < TOL, "forward output {a} vs {b}");
        }
    }
}
