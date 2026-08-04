// Dynamic recursive entry cannot be measured — requires #[max_depth(N)].
fn noise() -> int {
    return 10;
}

fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }
    return fib(n - 1) + fib(n - 2);
}

fn main() {
    let _ = fib(noise());
}
