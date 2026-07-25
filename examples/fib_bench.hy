// CPU / dispatch regression bench (release `poop` / perf_metrics).
// Uses fib(10) so the recursion shape is exercised without long wall time.
fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }

    return fib(n - 1) + fib(n - 2);
}

fn main() {
    print "%i", fib(10);
}
