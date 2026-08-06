use collections::map::{HashSet};

test("hashset basics") {
    let s = HashSet::new();
    assert(s.insert("a"))?;
    assert(s.insert("b"))?;
    assert(!s.insert("a"))?;
    assert(s.size() == 2)?;
    assert(s.contains("b"))?;
    assert(!s.contains("c"))?;
    assert(s.remove("a"))?;
    assert(!s.contains("a"))?;
    s.clear();
    assert(s.is_empty())?;
}

// Nested `self.inner.insert` exercises method-receiver staging + dict ABI.
test("hashset remove missing and reuse") {
    let s = HashSet::new();
    assert(s.remove(1) == false)?;
    assert(s.insert(1))?;
    assert(s.insert(2))?;
    assert(s.remove(1))?;
    assert(s.remove(1) == false)?;
    assert(s.contains(2))?;
    assert(s.size() == 1)?;
    s.clear();
    assert(s.insert(3))?;
    assert(s.contains(2) == false)?;
    assert(s.contains(3))?;
}
