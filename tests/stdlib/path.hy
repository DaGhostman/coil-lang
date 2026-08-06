use path::{join, dirname, basename, extension, is_absolute};

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

test("join empty sides") {
    let j = match join("a", "") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "join empty b",
    };
    assert(j == "a")?;
    let j2 = match join("", "b") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "join empty a",
    };
    assert(j2 == "b")?;
}

test("dirname basename extension edges") {
    let d = match dirname("plain") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "dirname plain",
    };
    assert(d == ".")?;
    let b = match basename("plain") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "basename plain",
    };
    assert(b == "plain")?;
    let e = match extension("noext") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "extension none",
    };
    assert(e == "")?;
    let root = match dirname("/") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "dirname root",
    };
    assert(root == "/")?;
}
