// Mutual recursion has no self-measure — requires #[max_depth(N)].
fn ping(int n) -> int {
    if n <= 0 {
        return 0;
    }
    return pong(n - 1);
}

fn pong(int n) -> int {
    if n <= 0 {
        return 1;
    }
    return ping(n - 1);
}

fn main() {
    let _ = ping(3);
}
