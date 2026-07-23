# entheai — container image (published to ghcr.io/entropy-om/entheai as a GitHub Package).
#
# A headless build of the CLI. Interactive TUI + local models still want a real
# terminal/host, but the container is handy for CI, `--serve` federation workers,
# and one-shot `entheai --no-companion "<prompt>"` runs.
#
# NOTE: first CI build validates this — adjust apt deps / feature flags if it fails.

# ---- build stage ---------------------------------------------------------------
FROM rust:1-bookworm AS build
WORKDIR /src
# System build deps (rustls avoids OpenSSL; cmake/clang cover native-dep builds).
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config cmake clang \
    && rm -rf /var/lib/apt/lists/*
# rust-toolchain.toml pins the exact toolchain; copy it first for a stable layer.
COPY rust-toolchain.toml ./
RUN rustup show
COPY . .
# Build just the primary binary in release, headless (drops GUI/companion and
# radio/alsa deps — this container has no display and no audio hardware).
RUN cargo build --release --locked --no-default-features -p entheai

# ---- runtime stage -------------------------------------------------------------
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 10001 entheai
COPY --from=build /src/target/release/entheai /usr/local/bin/entheai
USER entheai
WORKDIR /work
ENTRYPOINT ["entheai"]
CMD ["--help"]
