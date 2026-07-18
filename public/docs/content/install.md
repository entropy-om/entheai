---
id: install
title: "Install & build"
group: "Getting started"
order: 1
badgeText: "Getting started"
---

Clone the repo and build a release binary. You'll need a recent Rust toolchain.

```bash
git clone https://github.com/peterlodri-sec/entheai
cd entheai
cargo build --release

./target/release/entheai --version
```

> [!TIP]
> Add `target/release` to your `PATH` so you can call `entheai` from anywhere.
