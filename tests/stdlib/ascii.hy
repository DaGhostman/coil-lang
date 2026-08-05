use ascii::{is_space, is_digit, is_alpha, digit_val, digit_char, is_hex, hex_val, hex_digit};

test("ascii classification") {
    assert(is_space(" "))?;
    assert(is_space("\t"))?;
    assert(is_space("\n"))?;
    assert(!is_space("A"))?;
    assert(is_digit("0"))?;
    assert(is_digit("9"))?;
    assert(!is_digit("/"))?;
    assert(is_alpha("A"))?;
    assert(is_alpha("z"))?;
    assert(!is_alpha("7"))?;
}

test("ascii decimal digits") {
    assert(digit_val("0") == 0)?;
    assert(digit_val("7") == 7)?;
    assert(digit_val("x") == -1)?;
    assert(digit_char(0) == "0")?;
    assert(digit_char(9) == "9")?;
}

test("ascii hex digits") {
    assert(is_hex("a"))?;
    assert(is_hex("F"))?;
    assert(!is_hex("g"))?;
    assert(hex_val("a") == 10)?;
    assert(hex_val("F") == 15)?;
    assert(hex_val("g") == -1)?;
    assert(hex_digit(10) == "a")?;
    assert(hex_digit(0) == "0")?;
}
