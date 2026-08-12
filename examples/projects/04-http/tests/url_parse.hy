// Pure unit tests: URL parse (no sockets).
use http::url::{parse_url};

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

test("default path when omitted") {
    let u = match parse_url("http://example.com") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(u.path == "/", "default path")?;
    assert(u.port == 80, "default http port")?;
}

test("https defaults to port 443") {
    let u = match parse_url("https://example.com/secure") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(u.port == 443, "default https port")?;
    assert(u.scheme == "https", "scheme")?;
    assert(u.path == "/secure", "path")?;
}

test("query path with explicit slash") {
    let u = match parse_url("http://example.com/?q=1") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(u.path == "/?q=1", "query path")?;
}

test("reject unsupported scheme") {
    let r = parse_url("ftp://example.com/file");
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected unsupported scheme Err")?;
}

test("reject empty host") {
    let r = parse_url("http://");
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected empty host Err")?;
}

test("reject non-numeric port") {
    let r = parse_url("http://example.com:abc/");
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected bad port Err")?;
}

test("reject url path with crlf") {
    let r = parse_url("http://example.com/evil\r\nHost: x");
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected path CRLF BadUrl")?;
}

test("reject url host with crlf") {
    let r = parse_url("http://evil\rhost/");
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected host CR BadUrl")?;
}

test("reject url path with bare lf") {
    let r = parse_url("http://example.com/a\nb");
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected path LF BadUrl")?;
}
