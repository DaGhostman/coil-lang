// Comparison and logical operators.
test("int equality") {
    assert(1 == 1)?;
    assert(1 != 2)?;
    assert(!(1 == 2))?;
}

test("int ordering") {
    assert(1 < 2)?;
    assert(2 > 1)?;
    assert(1 <= 1)?;
    assert(2 >= 2)?;
    assert(3 >= 2)?;
    assert(!(5 < 3))?;
    assert(-5 < 0)?;
    assert(-1 <= -1)?;
    assert(!(-5 > 0))?;
}

test("float comparisons") {
    assert(1.0 < 2.0)?;
    assert(2.5 >= 2.5)?;
    assert(3.0 != 3.1)?;
}

test("bool logical and or") {
    assert(true && true)?;
    assert(!(true && false))?;
    assert(true || false)?;
    assert(!(false || false))?;
}

test("logical not") {
    assert(!false)?;
    assert(!(!true))?;
    assert(!0)?;
    assert(!(!1))?;
}

test("string equality") {
    assert("hi" == "hi")?;
    assert("a" != "b")?;
}

test("comparison of computed values") {
    let x = 10;
    let y = 3;
    assert(x + y == 13)?;
    assert(x > y)?;
    assert(x % y == 1)?;
}
