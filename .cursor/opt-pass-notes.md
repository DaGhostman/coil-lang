# Universal VM Opt Pass — measurement notes

## T0 (preflight, ARCHIVE 30, after merge #70 + rewrite_top CALL)

| bench | wall | instructions | cache_misses | branch_misses |
|-------|------|--------------|--------------|---------------|
| fib_bench | 1.98ms | 1.69M | 15.1K | 14.8K |
| fib(32) | 36.8ms | 699M | 17.3K | 205K |

perf_metrics: pass
