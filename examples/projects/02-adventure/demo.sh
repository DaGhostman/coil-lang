#!/usr/bin/env bash
# Play or CI-drive the adventure demo.
#
#   ./demo.sh           # interactive (type commands, then Ctrl+D)
#   ./demo.sh --ci      # pipe transcript.txt under timeout
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="${BIN:-$CARGO_TARGET_DIR/release/zero-script}"
TIMEOUT_SECS="${TIMEOUT_SECS:-10}"
ENTRY="$HERE/src/main.0s"

if [[ ! -x "$BIN" ]]; then
  echo "Building release zero-script…"
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
fi

rm -f "$ROOT/out.c0s" "$HERE/out.c0s"

case "${1:-}" in
  --ci | -c | "")
    if [[ "${1:-}" == "--ci" || "${1:-}" == "-c" ]]; then
      timeout "${TIMEOUT_SECS}s" "$BIN" "$ENTRY" <"$HERE/transcript.txt"
    else
      # Default: interactive when stdin is a TTY, else treat as CI pipe.
      if [[ -t 0 ]]; then
        exec "$BIN" "$ENTRY"
      else
        timeout "${TIMEOUT_SECS}s" "$BIN" "$ENTRY"
      fi
    fi
    ;;
  --help | -h)
    cat <<'EOF'
Usage: demo.sh [--ci]

  (no args, TTY)   interactive adventure (end with Ctrl+D)
  (no args, pipe)  read stdin under timeout
  --ci / -c        pipe transcript.txt under timeout
EOF
    ;;
  *)
    echo "Unknown option: $1 (try --help)" >&2
    exit 2
    ;;
esac
