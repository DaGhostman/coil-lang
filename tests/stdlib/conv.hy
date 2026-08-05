use conv::{int_to_dec, parse_int, parse_float};

test("decimal integer format and parse") {
    assert(int_to_dec(0) == "0")?;
    assert(int_to_dec(42) == "42")?;
    assert(int_to_dec(-907) == "-907")?;
    assert(parse_int("123")? == 123)?;
    assert(parse_int("+8")? == 8)?;
    assert(parse_int("-45")? == -45)?;
}

test("invalid decimal integer") {
    let failed = match parse_int("12x") {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    };
    assert(failed)?;
}

test("decimal float parse fraction") {
    assert(parse_float("12.5")? == 12.5)?;
}

test("decimal float parse leading dot") {
    assert(parse_float("-.25")? == 0.0 - 0.25)?;
}

test("decimal float parse positive exponent") {
    assert(parse_float("1.5e2")? == 150.0)?;
}

test("decimal float parse negative exponent") {
    let value = parse_float("2E-1")?;
    assert(value > 0.19)?;
    assert(value < 0.21)?;
}
