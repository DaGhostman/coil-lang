// Multi-param trait + where clause (Phase 3).
// Convert<A, B> with an int→int identity instance; cast(42) → 42.

trait Convert<A, B> {
    fn cast(A x) -> B;
}

impl Convert<int, int> {
    fn cast(int x) -> int {
        return x;
    }
}

fn apply_cast<A, B>(A x) -> B where Convert<A, B> {
    return cast(x);
}

fn main() {
    print "%i", apply_cast(42);
}
