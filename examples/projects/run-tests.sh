#!/usr/bin/env bash
# Run co-located `zero-script test` suites for each showcase project.
# Harness only scans ./tests relative to CWD — hence the per-project cd.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

BIN="${BIN:-$CARGO_TARGET_DIR/release/zero-script}"
TIMEOUT_SECS="${TIMEOUT_SECS:-60}"
PROJECTS="$ROOT/examples/projects"

if [[ ! -x "$BIN" ]]; then
  echo "Building release zero-script…"
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
fi

failed=0
for proj in 01-todo 02-adventure 03-echo; do
  echo "=== $proj tests ==="
  rm -f "$PROJECTS/$proj/out.c0s" "$ROOT/out.c0s"
  if (
    cd "$PROJECTS/$proj"
    timeout "${TIMEOUT_SECS}s" "$BIN" test
  ); then
    echo
  else
    echo "FAILED: $proj" >&2
    failed=1
    echo
  fi
done

if [[ "$failed" -ne 0 ]]; then
  echo "One or more showcase test suites failed." >&2
  exit 1
fi
echo "All showcase project tests passed."
