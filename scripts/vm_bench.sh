#!/usr/bin/env bash
# VM measurement + correctness harness (Phase VM instruction-count reduction).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

BIN="${BIN:-$CARGO_TARGET_DIR/release/zero-script}"
MEM_LIMIT_KB="${MEM_LIMIT_KB:-65536}"

declare -A EXPECTED=(
    ["examples/fib.0s"]="2178309"
    ["examples/option.0s"]="42"
    ["examples/result.0s"]="420-1"
    ["examples/tree.0s"]="6"
    ["examples/mixed.0s"]="025122"
    ["examples/record.0s"]="169512"
    ["examples/let_test.0s"]="51020"
    ["examples/chained.0s"]="427"
    ["examples/dict.0s"]="4210042"
    ["examples/aliases.0s"]="347"
    ["examples/fizbuz.0s"]="FIZBUZFIZFIZBUZFIZFIZBUZ"
    ["examples/operators.0s"]="801125428falsetrue3"
    ["examples/perf/numeric.0s"]="1999000"
    ["examples/perf/array_mut.0s"]="2000"
    ["examples/perf/dict_hot.0s"]="6000"
    ["examples/perf/operators_loop.0s"]="149912"
    ["examples/perf/coro_ping.0s"]="124750"
)

# CPU-focused subset for poop / quick timing (no FFI, no modules).
CPU_BENCH=(
    examples/fib.0s
    examples/perf/numeric.0s
    examples/perf/operators_loop.0s
    examples/perf/match_sum.0s
)

run_example() {
    local path="$1"
    rm -f out.c0s
    ulimit -v "$MEM_LIMIT_KB"
    "$BIN" "$path"
}

echo "== Building release binary =="
RUSTC_WRAPPER= cargo build --release

echo
echo "== Example stdout correctness (ulimit -v ${MEM_LIMIT_KB}) =="
fail=0
for path in "${!EXPECTED[@]}"; do
    want="${EXPECTED[$path]}"
    got="$(run_example "$path" 2>/dev/null || true)"
    if [[ "$got" == "$want" ]]; then
        echo "OK  $path -> $got"
    else
        echo "FAIL $path"
        echo "  expected: $want"
        echo "  got:      $got"
        fail=1
    fi
done

if [[ "$fail" -ne 0 ]]; then
    echo "Example correctness: FAILED"
    exit 1
fi
echo "Example correctness: PASSED"

if command -v poop >/dev/null 2>&1; then
    echo
    echo "== poop benchmark (fib vs lua) =="
    rm -f out.c0s
    poop -d 6000 "$BIN examples/fib.0s" "lua benchmarks/test.lua" || true
    echo
    echo "== poop CPU bench subset =="
    for path in "${CPU_BENCH[@]}"; do
        rm -f out.c0s
        echo "-- $path"
        poop -d 3000 "$BIN $path" || true
    done
else
    echo
    echo "poop not installed; skipping instruction-count benchmark"
fi

echo
echo "== Release binary size =="
ls -lh "$BIN"

if command -v valgrind >/dev/null 2>&1; then
    echo
    echo "== callgrind on fib.0s =="
    rm -f out.c0s callgrind.out.*
    valgrind --tool=callgrind --callgrind-out-file=callgrind.out "$BIN" examples/fib.0s >/dev/null 2>&1 || true
    if command -v callgrind_annotate >/dev/null 2>&1; then
        callgrind_annotate callgrind.out 2>/dev/null | head -20 || true
    fi
else
    echo
    echo "valgrind not installed; skipping callgrind"
fi

echo
echo "Done."
