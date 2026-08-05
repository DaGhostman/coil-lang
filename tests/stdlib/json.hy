use json::{
    parse, stringify, json_int, json_float, json_object, object_get, Json,
};

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
    let got = object_get(o, "a");
    match got {
        Option::Some(jv) => {
            let ss = match stringify(jv) {
                Result::Ok(x) => x,
                Result::Err(_) => panic "get",
            };
            assert(ss == "1")?;
        },
        Option::None => panic "missing a",
    };
}

test("array roundtrip") {
    let v = match parse("[1,2,3]") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "parse arr",
    };
    let s = match stringify(v) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "stringify",
    };
    assert(s == "[1,2,3]")?;
}

test("array bools") {
    let v = match parse("[true,false]") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "parse bools",
    };
    let s = match stringify(v) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "stringify bools",
    };
    assert(s == "[true,false]")?;
}

test("string escapes roundtrip") {
    let v = match parse("\"a\\\"b\\\\c\"") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "parse esc",
    };
    let s = match stringify(v) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "stringify esc",
    };
    assert(s == "\"a\\\"b\\\\c\"")?;
}

test("false scalar") {
    let v = match parse("false") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "false",
    };
    let s = match stringify(v) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "stringify false",
    };
    assert(s == "false")?;
}

test("unicode escape and floats") {
    let v = match parse("\"\\u0041\\u00e9\"") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "parse u",
    };
    let s = match stringify(v) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "stringify u",
    };
    assert(s == "\"Aé\"")?;
    let arr = match parse("[1.5,2.0,-3]") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "parse floats",
    };
    let as = match stringify(arr) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "stringify floats",
    };
    assert(as == "[1.5,2,-3]")?;
    let f = match stringify(json_float(1.25)) {
        Result::Ok(x) => x,
        Result::Err(_) => panic "float",
    };
    assert(f == "1.25")?;
}

test("nested object float") {
    let v = match parse("{\"n\":1.5,\"ok\":true}") {
        Result::Ok(x) => x,
        Result::Err(_) => panic "obj",
    };
    let n = object_get(v, "n");
    match n {
        Option::Some(jv) => {
            let ss = match stringify(jv) {
                Result::Ok(x) => x,
                Result::Err(_) => panic "n",
            };
            assert(ss == "1.5")?;
        },
        Option::None => panic "missing n",
    };
}
