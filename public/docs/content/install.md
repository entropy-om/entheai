---
id: install
title: "Install & build"
group: "Getting started"
order: 1
badgeText: "Getting started"
---

`entheai` is native to macOS on Apple Silicon (`aarch64-apple-darwin`).

## Option 1: Install via Homebrew

```bash
brew tap entropy-om/entheai https://github.com/entropy-om/entheai
brew trust entropy-om/entheai    # one-time security gate
brew install entheai
```

## Option 2: Build from Source

Requires a pinned Rust toolchain (`1.96.0`, MSRV `1.94`).

```bash
git clone https://github.com/entropy-om/entheai.git
cd entheai
cargo build --release
```

The release binary will be available at `./target/release/entheai`.

## Native App & Ghostty Integration

- Launch the native minimalist app window: `entheai --app` (requires Ghostty).
- Install the ambient rain-on-glass shader to your own Ghostty configuration: `entheai --doctor`.
- Reproducible release builds: `scripts/build-repro.sh --verify`.

> [!TIP]
> Run `./scripts/check.sh` after cloning to run the full formatting, clippy, and test suite.
