// COI-78: user-trait method at a ground type vs the same trait under a
// generic bound must agree on the result. Dispatch differs (CALL vs
// dictionary CallIndirect) but the ABI is one dictionary convention.

trait Measurable<T> {
    fn size(T x) -> int;
}

impl Measurable<int> {
    fn size(int x) -> int {
        return x + 1;
    }
}

fn size_of<T: Measurable>(T x) -> int {
    return x.size();
}

fn size_of_ufcs<T: Measurable>(T x) -> int {
    return size(x);
}

test("user trait at a ground type") {
    assert(41.size() == 42)?;
}

test("user trait under a generic bound") {
    assert(size_of(41) == 42)?;
    assert(size_of_ufcs(41) == 42)?;
}
