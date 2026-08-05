use num::{min, max, clamp};

#[derive(Ord, Eq)]
enum Rank {
    Low,
    Mid,
    High,
}

#[derive(Ord, Eq)]
enum Pair {
    Pair { x: int, y: int },
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

// Constructor sites refine to `::vN`; mixed variants must still join as the
// parent enum so `min`/`max`/`clamp` work without `: Rank` annotations.
test("Ord helpers accept bare mixed variant constructors") {
    assert(min(Rank::Mid, Rank::Low) == Rank::Low)?;
    assert(max(Rank::Mid, Rank::High) == Rank::High)?;
    assert(clamp(Rank::High, Rank::Low, Rank::Mid) == Rank::Mid)?;
}

test("Ord helpers on record-payload enum constructors") {
    let a = Pair::Pair { x: 1, y: 2 };
    let b = Pair::Pair { x: 1, y: 3 };
    let c = Pair::Pair { x: 2, y: 0 };
    assert(min(a, b) == a)?;
    assert(max(a, c) == c)?;
    assert(clamp(c, a, b) == b)?;
}
