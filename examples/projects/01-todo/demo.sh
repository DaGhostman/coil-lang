#!/usr/bin/env bash
# Run the 01-todo showcase demo.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="${BIN:-$CARGO_TARGET_DIR/release/zero-script}"
TIMEOUT_SECS="${TIMEOUT_SECS:-10}"

if [[ ! -x "$BIN" ]]; then
  echo "Building release zero-script…"
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
fi

rm -f "$ROOT/out.c0s" "$HERE/out.c0s"
timeout "${TIMEOUT_SECS}s" "$BIN" "$HERE/src/main.0s"
