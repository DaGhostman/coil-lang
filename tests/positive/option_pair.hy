fn parse_pair(int n, int fail) {
    if fail == 1 {
        raise "bad";
    }
    return n;
}

fn match_pair(int fail) {
    return match parse_pair(7, fail) {
        Result::Ok(value) => value,
        Result::Err(_) => -1,
    };
}

fn chain_pair(int fail) {
    let value = parse_pair(7, fail)?;
    return value + 1;
}

fn pass_value<T>(T value) -> T {
    return value;
}

fn generic_option(Option value) -> string {
    return pass_value(value) ?? "none";
}

test("direct pair match") {
    assert(match_pair(0) == 7)?;
    assert(match_pair(1) == -1)?;
}

test("pair try propagation") {
    assert(chain_pair(0) == 8)?;
    assert(match chain_pair(1) {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    })?;
}

test("pointer niche option from Vec pop") {
    let values = Vec::from(["a", "b"]);
    let last = match values.pop() {
        Option::Some(value) => value,
        Option::None => "none",
    };
    assert(last == "b")?;
    let _ = values.pop();
    let empty = match values.pop() {
        Option::Some(_) => false,
        Option::None => true,
    };
    assert(empty)?;
}

test("generic Option boundary") {
    assert(generic_option(Option::Some("ok")) == "ok")?;
    assert(generic_option(Option::None) == "none")?;
}
