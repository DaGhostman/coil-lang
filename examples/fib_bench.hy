// CPU / dispatch regression bench (release `poop` / perf_metrics).
// fib(32) exercises auto-par const specialization when enabled.
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }

    return fib(n - 1) + fib(n - 2);
}

fn main() {
    write_all(stdout(), to_bytes(format("%i", fib(32))));
}
