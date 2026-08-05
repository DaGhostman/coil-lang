use num::{min, max, clamp};

#[derive(Ord, Eq)]
enum Rank {
    Low,
    Mid,
    High,
}

// Cross-module `T: Ord` helpers must keep the real poly scheme + dict ABI
// after `use num::{…}` (bare type-param args are boxed for the shared body).
test("imported Ord min max clamp on derived enum") {
    let a: Rank = Rank::Mid;
    let b: Rank = Rank::Low;
    let c: Rank = Rank::High;
    assert(min(a, b) == Rank::Low)?;
    assert(max(a, c) == Rank::High)?;
    assert(clamp(b, a, c) == Rank::Mid)?;
    assert(clamp(c, b, a) == Rank::Mid)?;
}
