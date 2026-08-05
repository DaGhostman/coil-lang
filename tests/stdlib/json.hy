use json::*;

test("parse scalars") {
    let v = match parse("null") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "null",
    };
    let s = match stringify(v) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "s",
    };
    assert(s == "null")?;
    let v2 = match parse("true") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "true",
    };
    let s2 = match stringify(v2) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "s2",
    };
    assert(s2 == "true")?;
    let v3 = match parse("42") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "42",
    };
    let s3 = match stringify(v3) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "s3",
    };
    assert(s3 == "42")?;
}

test("stringify object") {
    let keys: [string] = [];
    keys[] = "a";
    let vals: [Json] = [];
    vals[] = json_int(1);
    let o = json_object(keys, vals);
    let s2 = match stringify(o) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "stringify obj",
    };
    assert(len(s2) == 7)?;
}

test("array roundtrip") {
    let v = match parse("[1,2,true]") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "parse arr",
    };
    let s = match stringify(v) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "stringify",
    };
    assert(s == "[1,2,true]")?;
}
