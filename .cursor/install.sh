#!/usr/bin/env bash
# Idempotent repository bootstrap for coil-lang Cloud Agents.
# Refreshes the userland stdlib sibling and warms the workspace build so the
# `coil` binary and cargo caches are ready. Safe to re-run against cached state.
set -euo pipefail

# coil.toml module roots include ./.deps/coil-stdlib/src (io::sync + showcase
# modules). The repo is public, so an unauthenticated shallow clone is enough.
STDLIB_DIR=".deps/coil-stdlib"
STDLIB_URL="https://github.com/ardax-corp/coil-stdlib.git"
if [ -d "$STDLIB_DIR/.git" ]; then
    git -C "$STDLIB_DIR" fetch --depth 1 origin HEAD
    git -C "$STDLIB_DIR" reset --hard FETCH_HEAD
else
    rm -rf "$STDLIB_DIR"
    git clone --depth 1 "$STDLIB_URL" "$STDLIB_DIR"
fi

# Warm the workspace build; also produces ./target/debug/coil used by the
# `coil test` language harness and showcase project tests.
cargo build
