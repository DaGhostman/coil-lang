#!/usr/bin/env bash
# Soft CPU baseline for stack-IL / opt changes (see AGENTS.md).
#
# Not a hard CI gate: prints poop stats for fib_bench so regressions are
# easy to spot before/after compiler changes. Requires `poop` on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="${BIN:-$CARGO_TARGET_DIR/release/coil}"
DURATION_MS="${DURATION_MS:-6000}"

if ! command -v poop >/dev/null 2>&1; then
    echo "poop not installed; install it or skip this baseline" >&2
    exit 1
fi

echo "== Building release binary =="
RUSTC_WRAPPER= cargo build --release --quiet

rm -f out.hyc
echo "== poop -d ${DURATION_MS} fib_bench.hy =="
poop -d "$DURATION_MS" "$BIN examples/fib_bench.hy"
rm -f out.hyc
