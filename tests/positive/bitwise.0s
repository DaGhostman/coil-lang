// Bitwise operators and shifts.
test("bitwise and or xor") {
    assert((7 & 3) == 3)?;
    assert((4 | 1) == 5)?;
    assert((7 ^ 3) == 4)?;
}

test("bitwise not") {
    assert((~0) == -1)?;
    assert((~(-1)) == 0)?;
}

test("shifts") {
    assert((1 << 3) == 8)?;
    assert((16 >> 2) == 4)?;
    assert((7 << 1) == 14)?;
}

test("compound bitwise") {
    let x = 15;
    x &= 7;
    assert(x == 7)?;
    x |= 8;
    assert(x == 15)?;
    x ^= 1;
    assert(x == 14)?;
    x <<= 1;
    assert(x == 28)?;
    x >>= 2;
    assert(x == 7)?;
}
