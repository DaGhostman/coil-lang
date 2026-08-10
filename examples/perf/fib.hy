// CPU: plain naive fib recursion (cross-lang fair bench).
// Compile with COIL_AUTO_PAR=0 so the binary fork-join is not specialized.
// Checksum: fib(32) = 2178309.
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
