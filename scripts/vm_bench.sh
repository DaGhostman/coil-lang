#!/usr/bin/env bash
# VM measurement + correctness harness (Phase VM instruction-count reduction).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

BIN="${BIN:-$CARGO_TARGET_DIR/release/coil}"
MEM_LIMIT_KB="${MEM_LIMIT_KB:-65536}"

declare -A EXPECTED=(
    ["examples/fib.hy"]="55"
    ["examples/fib_bench.hy"]="2178309"
    ["examples/option.hy"]="42"
    ["examples/result.hy"]="420-1"
    ["examples/tree.hy"]="6"
    ["examples/mixed.hy"]="025122"
    ["examples/record.hy"]="169512"
    ["examples/let_test.hy"]="51020"
    ["examples/chained.hy"]="427"
    ["examples/dict.hy"]="4210042"
    ["examples/aliases.hy"]="347"
    ["examples/fizbuz.hy"]="FIZBUZFIZFIZBUZFIZFIZBUZ"
    ["examples/operators.hy"]="801125428falsetrue3"
    ["examples/perf/numeric.hy"]="1999000"
    ["examples/perf/array_mut.hy"]="2000"
    ["examples/perf/dict_hot.hy"]="6000"
    ["examples/perf/operators_loop.hy"]="149912"
    ["examples/perf/coro_ping.hy"]="124750"
)

# CPU-focused subset for poop / quick timing (no FFI, no modules).
CPU_BENCH=(
    examples/fib_bench.hy
    examples/perf/numeric.hy
    examples/perf/operators_loop.hy
    examples/perf/match_sum.hy
)

run_example() {
    local path="$1"
    rm -f out.hyc
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
    rm -f out.hyc
    poop -d 6000 "$BIN examples/fib_bench.hy" "lua benchmarks/test.lua" || true
    echo
    echo "== poop CPU bench subset =="
    for path in "${CPU_BENCH[@]}"; do
        rm -f out.hyc
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
    echo "== callgrind on fib_bench.hy =="
    rm -f out.hyc callgrind.out.*
    valgrind --tool=callgrind --callgrind-out-file=callgrind.out "$BIN" examples/fib_bench.hy >/dev/null 2>&1 || true
    if command -v callgrind_annotate >/dev/null 2>&1; then
        callgrind_annotate callgrind.out 2>/dev/null | head -20 || true
    fi
else
    echo
    echo "valgrind not installed; skipping callgrind"
fi

echo
echo "Done."
