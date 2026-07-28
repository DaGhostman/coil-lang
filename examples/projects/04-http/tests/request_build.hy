use io::*;
use http::url::*;

fn contains_ascii(string hay, string needle) -> int {
    let h = to_bytes(hay);
    let n = to_bytes(needle);
    let hn = len(h);
    let nn = len(n);
    if nn == 0 { return 1; }
    if nn > hn { return 0; }
    let i = 0;
    while i + nn <= hn {
        let ok = 1;
        let j = 0;
        while j < nn {
            if h[i + j] != n[j] { ok = 0; }
            j = j + 1;
        }
        if ok == 1 { return 1; }
        i = i + 1;
    }
    return 0;
}

test("build get request has host and connection close") {
    let u = match parse_url("http://example.com/hi") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    let msg = match build_request_head("GET", u, hs, 0) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    if len(msg) < 40 { panic "short"; }
    let text = match from_bytes(msg) {
        Result::Ok(s) => s,
        Result::Err(_) => panic "utf8",
    };
    // Byte checks (string contains can disagree with CR in haystack)
    let getb = to_bytes("GET /hi HTTP/1.1");
    let hostb = to_bytes("Host: example.com");
    let closeb = to_bytes("Connection: close");
    let clb = to_bytes("Content-Length: 0");
    assert(find_bytes(msg, getb) != 999999, "request line")?;
    assert(find_bytes(msg, hostb) != 999999, "host")?;
    assert(find_bytes(msg, closeb) != 999999, "connection")?;
    assert(find_bytes(msg, clb) != 999999, "content-length")?;
}

test("build post sets content-length") {
    let u = match parse_url("http://example.com/") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    let msg = match build_request_head("POST", u, hs, 2) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    let clb = to_bytes("Content-Length: 2");
    assert(find_bytes(msg, clb) != 999999, "content-length")?;
}
