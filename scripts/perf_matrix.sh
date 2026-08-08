#!/usr/bin/env bash
# Capture repeatable AOT and cross-language performance artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

BIN="${BIN:-$CARGO_TARGET_DIR/release/coil}"
DURATION_MS="${DURATION_MS:-6000}"
OUT_DIR="${OUT_DIR:-/tmp/coil_perf_matrix}"
RUN_MASSIF="${RUN_MASSIF:-0}"

CROSS_LANG=(mandelbrot tak nsieve binary_trees)
AOT_ONLY=(numeric operators_loop match_sum)
declare -A EXPECTED=(
    [mandelbrot]=625885
    [tak]=7
    [nsieve]=1900
    [binary_trees]=135854
    [numeric]=1999000
    [operators_loop]=149912
    [match_sum]=7995
)

if ! command -v poop >/dev/null 2>&1; then
    echo "poop is required for the performance matrix" >&2
    exit 1
fi
for tool in lua node; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "$tool is required for the cross-language matrix" >&2
        exit 1
    }
done

mkdir -p "$OUT_DIR"
{
    printf '# Coil performance matrix\n\n'
    printf -- '- timestamp: `%s`\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf -- '- commit: `%s`\n' "$(git rev-parse HEAD)"
    printf -- '- rustc: `%s`\n' "$(rustc --version)"
    printf -- '- duration: `%sms` per poop comparison\n\n' "$DURATION_MS"
} >"$OUT_DIR/README.md"

RUSTC_WRAPPER= cargo build --release --quiet

run_pooped() {
    local name="$1"
    shift
    local output="$OUT_DIR/${name}.poop.txt"
    {
        printf '## %s\n\n' "$name"
        poop -d "$DURATION_MS" "$@"
    } | tee "$output" >>"$OUT_DIR/README.md"
    printf '\n' >>"$OUT_DIR/README.md"
}

check_archive() {
    local name="$1"
    local archive="$2"
    local got
    got="$("$BIN" run "$archive")"
    if [[ "$got" != "${EXPECTED[$name]}" ]]; then
        echo "checksum mismatch for $name: expected ${EXPECTED[$name]}, got $got" >&2
        return 1
    fi
    printf -- '- checksum `%s`: `%s`\n' "$name" "$got" >>"$OUT_DIR/README.md"
}

for name in "${CROSS_LANG[@]}"; do
    archive="$OUT_DIR/${name}.hyc"
    "$BIN" compile "examples/perf/${name}.hy" -o "$archive" >/dev/null
    touch "$archive"
    check_archive "$name" "$archive"
    run_pooped "$name" \
        "$BIN run $archive" \
        "lua benchmarks/${name}.lua" \
        "node benchmarks/${name}.js"

    if [[ "$RUN_MASSIF" == 1 ]] && command -v valgrind >/dev/null 2>&1; then
        valgrind --tool=massif \
            --massif-out-file="$OUT_DIR/${name}.massif.out" \
            "$BIN" run "$archive" >/dev/null 2>&1 || true
    fi
done

for name in "${AOT_ONLY[@]}"; do
    archive="$OUT_DIR/${name}.hyc"
    "$BIN" compile "examples/perf/${name}.hy" -o "$archive" >/dev/null
    touch "$archive"
    check_archive "$name" "$archive"
    run_pooped "$name" "$BIN run $archive"
done

echo "Performance artifacts written to $OUT_DIR"
