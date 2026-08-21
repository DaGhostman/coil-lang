# Cloud Agent base image for coil-lang.
# Pins the latest stable Rust 1.x (edition 2024 needs >= 1.85) on Debian
# bookworm and bakes in the system + perf tooling AGENTS.md expects.
FROM rust:1-bookworm

# libffi: coil FFI / host natives link against it (matches CI `libffi-dev`).
# valgrind/heaptrack/hyperfine/lua5.4: perf + benchmark tooling per AGENTS.md.
# git/curl/ca-certificates: repo + stdlib checkout and release-binary download.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        libffi-dev \
        pkg-config \
        build-essential \
        valgrind \
        heaptrack \
        hyperfine \
        lua5.4 \
        ca-certificates \
        curl \
        git \
    && rm -rf /var/lib/apt/lists/*

# poop: perf comparison tool used by scripts/poop_baseline.sh (prebuilt binary).
RUN curl -fsSL -o /usr/local/bin/poop \
        https://github.com/andrewrk/poop/releases/download/0.5.0/x86_64-linux-poop \
    && chmod +x /usr/local/bin/poop

# Lint/format components used during contribution (cargo check is the CI gate).
RUN rustup component add clippy rustfmt
