// Pure unit tests: URL parse (no sockets).
use http::url::*;

test("parse http url with defaults") {
    let u = match parse_url("http://example.com/path") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(u.port == 80, "default http port")?;
    assert(u.scheme == "http", "scheme")?;
    assert(u.host == "example.com", "host")?;
    assert(u.path == "/path", "path")?;
}

test("parse https url with port and query") {
    let u = match parse_url("https://localhost:8443/x?q=1") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(u.port == 8443, "port")?;
    assert(u.scheme == "https", "scheme")?;
    assert(u.host == "localhost", "host")?;
    assert(u.path == "/x?q=1", "path+query")?;
}

test("reject bad url") {
    let r = parse_url("not-a-url");
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected Err")?;
}
