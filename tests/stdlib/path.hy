use path::*;

test("join dirname basename extension") {
    let j = match join("a", "b") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "join",
    };
    assert(j == "a/b")?;
    let j2 = match join("a/", "b") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "join2",
    };
    assert(j2 == "a/b")?;
    let d = match dirname("/tmp/x") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "dirname",
    };
    assert(d == "/tmp")?;
    let b = match basename("/tmp/x.txt") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "basename",
    };
    assert(b == "x.txt")?;
    let e = match extension("/tmp/x.txt") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "ext",
    };
    assert(e == "txt")?;
    assert(is_absolute("/tmp"))?;
    assert(is_absolute("rel") == false)?;
}
