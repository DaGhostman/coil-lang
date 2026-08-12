#!/usr/bin/env bash
# Run the 04-http cleartext client demo (server + get).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="${BIN:-$CARGO_TARGET_DIR/release/coil}"
TIMEOUT_SECS="${TIMEOUT_SECS:-10}"

if [[ ! -x "$BIN" ]]; then
  echo "Building release coil…"
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
fi

fuser -k 41250/tcp 2>/dev/null || true
rm -f "$ROOT/out.hyc" "$HERE/out.hyc"

SRV_DIR="$(mktemp -d)"
CLI_DIR="$(mktemp -d)"
cleanup() {
  kill "$SPID" 2>/dev/null || true
  wait "$SPID" 2>/dev/null || true
  fuser -k 41250/tcp 2>/dev/null || true
  rm -rf "$SRV_DIR" "$CLI_DIR"
}
trap cleanup EXIT

# Absolute stdlib root so temp cwd still resolves http::*
cat > "$CLI_DIR/coil.toml" << EOF
[module]
roots = ["./src", "$ROOT/stdlib/src"]
EOF
mkdir -p "$CLI_DIR/src"
cp "$HERE/src/main.hy" "$CLI_DIR/src/main.hy"

(
  cd "$SRV_DIR"
  timeout "${TIMEOUT_SECS}s" "$BIN" "$HERE/src/server.hy"
) &
SPID=$!

# Wait until the listener is up
for _ in $(seq 1 50); do
  if netstat -ltn 2>/dev/null | grep -q '127.0.0.1:41250'; then
    break
  fi
  sleep 0.05
done

set +e
(
  cd "$CLI_DIR"
  timeout "${TIMEOUT_SECS}s" "$BIN" src/main.hy
)
CODE=$?
set -e
exit "$CODE"
