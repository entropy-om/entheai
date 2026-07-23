#!/usr/bin/env bash
# Reproducible release build (roadmap Phase 2.2) — the deterministic sibling of
# build-release.sh. Where the PGO path tunes for THIS machine (target-cpu=native,
# profile data), this one anchors every input so the same sources + same rustc
# yield the same bytes:
#
#   * fixed target        aarch64-apple-darwin (no host drift)
#   * fixed CPU baseline  apple-m1 (NOT native — chips must not change the bytes)
#   * path remapping      $PWD and $HOME never leak into the binary
#   * pinned time         SOURCE_DATE_EPOCH = the HEAD commit's committer time
#   * zeroed ar dates     ZERO_AR_DATE=1 (macOS static-archive timestamps)
#   * locked deps         cargo --locked (Cargo.lock is the truth)
#
# `--verify` builds TWICE into independent target dirs and compares SHA-256 —
# reproducibility is demonstrated empirically, never asserted
# (frozen/verification.md). The manifest records the exact rustc: byte equality
# is promised only for identical toolchains.
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="aarch64-apple-darwin"
BASELINE_CPU="apple-m1"
BINS=(entheai entheai-worker entheai-launch)

export SOURCE_DATE_EPOCH="$(git log -1 --format=%ct 2>/dev/null || echo 0)"
export ZERO_AR_DATE=1
REPRO_RUSTFLAGS="--remap-path-prefix ${PWD}=/entheai --remap-path-prefix ${HOME}=/home -C target-cpu=${BASELINE_CPU}"

build_into() { # $1 = target dir
  CARGO_TARGET_DIR="$1" RUSTFLAGS="$REPRO_RUSTFLAGS" \
    cargo build --release --locked --target "$TARGET" \
    $(printf -- '-p %s ' "${BINS[@]}")
}

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

manifest() { # $1 = target dir, $2 = out file
  {
    echo "{"
    echo "  \"schema\": \"entheai.repro.v1\","
    echo "  \"rustc\": \"$(rustc --version)\","
    echo "  \"target\": \"${TARGET}\","
    echo "  \"cpu_baseline\": \"${BASELINE_CPU}\","
    echo "  \"source_date_epoch\": ${SOURCE_DATE_EPOCH},"
    echo "  \"commit\": \"$(git rev-parse HEAD 2>/dev/null || echo unknown)\","
    echo "  \"sha256\": {"
    local first=1
    for b in "${BINS[@]}"; do
      [ $first -eq 1 ] || echo ","
      first=0
      printf '    "%s": "%s"' "$b" "$(sha "$1/${TARGET}/release/$b")"
    done
    echo
    echo "  }"
    echo "}"
  } > "$2"
}

if [ "${1:-}" = "--verify" ]; then
  echo "==> reproducibility verification: two independent builds…"
  build_into target-repro-a
  build_into target-repro-b
  status=0
  for b in "${BINS[@]}"; do
    a_sha="$(sha "target-repro-a/${TARGET}/release/$b")"
    b_sha="$(sha "target-repro-b/${TARGET}/release/$b")"
    if [ "$a_sha" = "$b_sha" ]; then
      echo "  ✅ $b  ${a_sha:0:16}…  (identical)"
    else
      echo "  ❌ $b  DIFFERS: ${a_sha:0:16}… vs ${b_sha:0:16}…"
      status=1
    fi
  done
  mkdir -p dist
  manifest target-repro-a dist/repro-manifest.json
  echo "==> manifest: dist/repro-manifest.json"
  [ $status -eq 0 ] && echo "✅ byte-reproducible (this toolchain: $(rustc --version))"
  exit $status
fi

echo "==> deterministic release build…"
build_into target-repro-a
mkdir -p dist
manifest target-repro-a dist/repro-manifest.json
echo "✅ built: target-repro-a/${TARGET}/release/{${BINS[*]// /,}}"
echo "   manifest: dist/repro-manifest.json"
echo "   verify reproducibility empirically:  ./scripts/build-repro.sh --verify"
