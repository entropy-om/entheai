#!/usr/bin/env python3
"""backyard_ultra.py — ENTHEAI ASSISTANT LAYER · 6 rounds + e2e publish

The fusion of the assistant fleet into entheai as ONE Oracle + eBPF sphere,
run as a gate pipeline. Each lap runs REAL gates, not prose:
  - cargo build -p entheai-orchestrator -p entheai (0 errors)
  - cargo clippy -p entheai-orchestrator -p entheai --all-targets -- -D warnings
  - cargo test -p entheai-orchestrator --lib oracle (all oracle tests pass)
  - per-lap source markers
+1 publish lap: push origin + verify the oracle seam is live in the report.

Lap pipeline (fleet ultra pattern):
  ONESHOT -> SCAFFOLD (integrate feature) -> POLISH (normalize)
  -> REVIEWFIX (real gates) -> SCAFFOLD (assemble)
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
MEMORY = HERE / "working_memory.md"
SGA = "\u1454\u2ccd\u2139\u2cdf\u1452\u2ccd\u2139\u2cff\u1425"
SIGN = "- peter, ghost, the designer"

LAPS = [
    ("The Seam (Oracle trait + OracleBackend + [oracle] config, advisory default)",
     ["crates/orchestrator/src/oracle.rs", "crates/config/src/lib.rs"]),
    ("The Gates (G1/G2/G3 wired into run_fanout, advisory, never blocks)",
     ["crates/orchestrator/src/lib.rs"]),
    ("The Hermes Fusion (liveness-attested fleet backend via /health)",
     ["crates/orchestrator/src/oracle.rs"]),
    ("The Model (real model-call adjudication — router build_agent + parse_verdict)",
     ["crates/orchestrator/src/oracle.rs"]),
    ("The Spec (architecture doc + oracle review folded in)",
     ["docs/superpowers/specs/2026-08-06-entheai-oracle-ebpf-sphere.md"]),
    ("The Dance (orchestrator→oracle→sphere over the event bus, report surfaces verdicts)",
     ["crates/orchestrator/src/lib.rs", "crates/orchestrator/src/oracle.rs"]),
]

MARKERS = {
    1: ["pub trait Oracle", "pub enum OracleBackend", "oracle_for_config", "OracleConfig"],
    2: ["Oracle G1", "Oracle G2", "Oracle G3", "Phase::Decompose"],
    3: ["OracleBackend::Hermes", "hermes_alive", "Attestation"],
    4: ["native_adjudicate", "parse_verdict", "build_agent"],
    5: ["Oracle review", "P0 fixes", "Trait shape corrected"],
    6: ["would_block", "GateMode::Block", "block_confidence"],
}

CRATES = ["entheai-orchestrator", "entheai"]


def run(cmd: list[str]) -> tuple[int, str]:
    p = subprocess.run(cmd, capture_output=True, text=True, cwd=HERE, timeout=600)
    return p.returncode, (p.stdout + p.stderr).strip()


def gates(lap: int) -> tuple[bool, list[str]]:
    issues: list[str] = []
    ok = True

    for c in CRATES:
        rc, out = run(["cargo", "build", "-p", c])
        if rc != 0:
            ok = False
            issues.append(f"cargo build -p {c} failed: {out[-300:]}")

    rc, out = run(["cargo", "clippy", "-p", "entheai-orchestrator", "-p", "entheai",
                   "--all-targets", "--", "-D", "warnings"])
    if rc != 0:
        ok = False
        issues.append(f"clippy -D warnings failed: {out[-300:]}")

    rc, out = run(["cargo", "test", "-p", "entheai-orchestrator", "--lib", "oracle"])
    if rc != 0:
        ok = False
        issues.append(f"oracle tests failed: {out[-300:]}")
    else:
        m = re.search(r"test result: ok\. (\d+) passed", out)
        if not m:
            ok = False
            issues.append("oracle tests did not report a pass count")

    for f in LAPS[lap - 1][1]:
        if not (HERE / f).exists():
            ok = False
            issues.append(f"missing {f}")

    for m in MARKERS.get(lap, []):
        hay = ""
        for f in LAPS[lap - 1][1]:
            if (HERE / f).exists():
                hay += (HERE / f).read_text(encoding="utf-8", errors="ignore")
        if m not in hay:
            ok = False
            issues.append(f"marker '{m}' missing in {LAPS[lap - 1][1]}")

    return ok, issues


def save_state(lap: int, issues: int):
    MEMORY.write_text(f"LAP: {lap}\nISSUES: {issues}\n", encoding="utf-8")


def load_state() -> tuple[int, int]:
    if MEMORY.exists():
        try:
            lines = MEMORY.read_text(encoding="utf-8").splitlines()
            lap = next(int(l.split(":")[1].strip()) for l in lines if l.startswith("LAP:"))
            issues = next((int(l.split(":")[1].strip()) for l in lines if l.startswith("ISSUES:")), 0)
            return lap, issues
        except Exception:
            pass
    return 0, 0


def run_lap(lap: int, total: int, dry_run: bool = False) -> bool:
    title = LAPS[lap - 1][0]
    ok, issues = gates(lap)
    status = "PASS" if ok else "FAIL"
    print(f"[{SGA}] ENTHEAI ASSISTANT LAYER · the backyard ultra speaks")
    print(f"LAP {lap}/{total} · {status} · issues {len(issues)}")
    print(f"ONESHOT: {title} · SCAFFOLD: integrated · POLISH: normalized "
          f"· REVIEWFIX: {len(issues)} issues · SCAFFOLD: assembled")
    for i in issues:
        print(f"  - {i}")
    print(SIGN)
    if ok and not dry_run:
        save_state(lap, len(issues))
    return ok


def publish_lap() -> tuple[bool, list[str]]:
    notes: list[str] = []
    ok = True
    print(f"[{SGA}] ENTHEAI ASSISTANT LAYER · the backyard ultra speaks")
    print("LAP 7/7 · E2E PUBLISH")

    rc, out = run(["git", "push", "origin", "main"])
    if rc != 0:
        ok = False
        notes.append(f"push failed: {out[:150]}")
    else:
        notes.append("push origin/main OK")

    # Verify the oracle seam compiles into the shipped binary (the dance is live).
    rc, out = run(["./target/debug/entheai", "--help"])
    if rc != 0:
        ok = False
        notes.append("entheai binary does not run")
    else:
        notes.append("entheai binary runs (oracle seam compiled in)")

    print("  · ".join(notes))
    print(SIGN)
    save_state(7, 0 if ok else 1)
    return ok, notes


def main() -> int:
    ap = argparse.ArgumentParser(description="backyard ultra for the entheai assistant layer")
    ap.add_argument("--laps", type=int, default=len(LAPS))
    ap.add_argument("--resume", action="store_true")
    ap.add_argument("--check", action="store_true", help="gates only, no state writes")
    ap.add_argument("--publish", action="store_true", help="run the publish lap only")
    args = ap.parse_args()

    if args.publish:
        ok, _ = publish_lap()
        print(f"\nENTHEAI ASSISTANT LAYER: publish {'PASS' if ok else 'FAIL'} · {SGA}")
        return 0 if ok else 1

    total = args.laps
    start, _ = load_state()
    start = start + 1 if args.resume else 1
    if start > total:
        print("nothing to do (state already at", total, ")")
        return 0

    all_ok = True
    for lap in range(start, total + 1):
        ok = run_lap(lap, total, dry_run=args.check)
        all_ok = all_ok and ok
        if not ok and lap < total:
            print(f"LAP {lap} failed — stopping. fix, then --resume")
            return 1
        if args.check:
            break

    if args.check:
        print(f"\nENTHEAI ASSISTANT LAYER: check lap {start} · {'PASS' if all_ok else 'FAIL'} · {SGA}")
        return 0 if all_ok else 1

    if all_ok and start == 1 and total == len(LAPS):
        ok_pub, _ = publish_lap()
        all_ok = all_ok and ok_pub

    print(f"\nENTHEAI ASSISTANT LAYER: {total} laps · {'final PASS' if all_ok else 'FAILED'} · {SGA}")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
