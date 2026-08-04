// Dynamic recursive entry cannot be measured — requires #[max_depth(N)].
fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }
    return fib(n - 1) + fib(n - 2);
}

fn main() {
    let k = 10;
    let _ = fib(k);
}
