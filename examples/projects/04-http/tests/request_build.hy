use string::{to_bytes};
use http::url::{
    empty_headers,
    find_bytes,
    headers_have_crlf,
    parse_url,
};
use http::request::{
    build_request_head,
    build_request_head_extras,
    concat_bytes,
    extras_sanitize,
    format_extra_headers_str,
    request_line_ok,
};

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
    let getb = to_bytes("GET /hi HTTP/1.1");
    let hostb = to_bytes("Host: example.com");
    let closeb = to_bytes("Connection: close");
    let clb = to_bytes("Content-Length: 0");
    if find_bytes(msg, getb) == 999999 { panic "request line"; }
    if find_bytes(msg, hostb) == 999999 { panic "host"; }
    if find_bytes(msg, closeb) == 999999 { panic "connection"; }
    if find_bytes(msg, clb) == 999999 { panic "content-length"; }
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
    if find_bytes(msg, clb) == 999999 { panic "content-length"; }
}

test("custom headers appear on the wire") {
    let u = match parse_url("http://example.com/") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    hs.add("X-Trace", "abc");
    hs.add("Accept", "text/plain");
    let extras = format_extra_headers_str(hs.names, hs.values);
    let msg = match build_request_head_extras("GET", u, extras, 0) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    let xb = to_bytes("X-Trace: abc");
    let ab = to_bytes("Accept: text/plain");
    let hostb = to_bytes("Host: example.com");
    let closeb = to_bytes("Connection: close");
    let clb = to_bytes("Content-Length: 0");
    if find_bytes(msg, xb) == 999999 { panic "x-trace"; }
    if find_bytes(msg, ab) == 999999 { panic "accept"; }
    if find_bytes(msg, hostb) == 999999 { panic "host"; }
    if find_bytes(msg, closeb) == 999999 { panic "connection"; }
    if find_bytes(msg, clb) == 999999 { panic "content-length"; }
}

test("post body concat grows by body length") {
    let u = match parse_url("http://example.com/") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    let head = match build_request_head("POST", u, hs, 2) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    let body: Vec<byte> = Vec::new();
    body.push(65);
    body.push(66);
    let msg = concat_bytes(head, body);
    if len(msg) != len(head) + 2 { panic "concat len"; }
}

test("non-default http port in Host header") {
    let u = match parse_url("http://example.com:8080/x") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    let msg = match build_request_head("GET", u, hs, 0) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    let hostb = to_bytes("Host: example.com:8080");
    let bare = to_bytes("Host: example.com\r\n");
    if find_bytes(msg, hostb) == 999999 { panic "host with port"; }
    if find_bytes(msg, bare) != 999999 { panic "bare host without port"; }
}

test("https default port omits port from Host") {
    let u = match parse_url("https://example.com/s") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    let msg = match build_request_head("GET", u, hs, 0) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    let hostb = to_bytes("Host: example.com\r\n");
    let with443 = to_bytes("Host: example.com:443");
    if find_bytes(msg, hostb) == 999999 { panic "host without default https port"; }
    if find_bytes(msg, with443) != 999999 { panic "must omit :443"; }
}

test("reserved headers skipped in extras") {
    let u = match parse_url("http://example.com/") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    hs.add("Host", "evil.example");
    hs.add("Content-Length", "999");
    hs.add("Connection", "keep-alive");
    hs.add("X-Ok", "1");
    let extras = format_extra_headers_str(hs.names, hs.values);
    let msg = match build_request_head_extras("GET", u, extras, 0) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    let okb = to_bytes("X-Ok: 1");
    let evil = to_bytes("Host: evil.example");
    let fake_cl = to_bytes("Content-Length: 999");
    let keep = to_bytes("Connection: keep-alive");
    let real_host = to_bytes("Host: example.com");
    let real_cl = to_bytes("Content-Length: 0");
    let closeb = to_bytes("Connection: close");
    if find_bytes(msg, okb) == 999999 { panic "custom header"; }
    if find_bytes(msg, evil) != 999999 { panic "must ignore caller Host"; }
    if find_bytes(msg, fake_cl) != 999999 { panic "must ignore caller Content-Length"; }
    if find_bytes(msg, keep) != 999999 { panic "must ignore caller Connection"; }
    if find_bytes(msg, real_host) == 999999 { panic "client Host"; }
    if find_bytes(msg, real_cl) == 999999 { panic "client Content-Length"; }
    if find_bytes(msg, closeb) == 999999 { panic "client Connection"; }
}

test("only reserved headers yield no extras") {
    let hs = empty_headers();
    hs.add("host", "evil");
    hs.add("content-length", "1");
    hs.add("connection", "keep-alive");
    let extras = format_extra_headers_str(hs.names, hs.values);
    assert(extras == "__NONE__", "sentinel when all reserved")?;
}

test("uppercase reserved header spellings are skipped") {
    let hs = empty_headers();
    hs.add("HOST", "evil");
    hs.add("CONTENT-LENGTH", "999");
    hs.add("CONNECTION", "keep-alive");
    hs.add("X-Ok", "1");
    let extras = format_extra_headers_str(hs.names, hs.values);
    let eb = to_bytes(extras);
    let okb = to_bytes("X-Ok: 1");
    let evil = to_bytes("evil");
    let fake = to_bytes("CONTENT-LENGTH");
    if find_bytes(eb, okb) == 999999 { panic "keeps custom"; }
    if find_bytes(eb, evil) != 999999 { panic "skips HOST"; }
    if find_bytes(eb, fake) != 999999 { panic "skips CONTENT-LENGTH"; }
}

test("lookup content-length sixteen") {
    let u = match parse_url("http://example.com/") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    let msg = match build_request_head("POST", u, hs, 16) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    let clb = to_bytes("Content-Length: 16");
    let closeb = to_bytes("Connection: close");
    if find_bytes(msg, clb) == 999999 { panic "content-length 16"; }
    if find_bytes(msg, closeb) == 999999 { panic "connection"; }
}

test("reject method with crlf") {
    let u = match parse_url("http://example.com/") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let hs = empty_headers();
    let msg = match build_request_head("GET\r\nX: y", u, hs, 0) {
        Result::Ok(m) => m,
        Result::Err(_) => panic "build",
    };
    assert(request_line_ok(msg) == 0, "injected method fails request-line check")?;
}

test("reject header name with crlf") {
    let names: Vec<string> = Vec::new();
    let values: Vec<string> = Vec::new();
    names.push("X-Evil\r\nHost");
    values.push("ok");
    assert(headers_have_crlf(names, values) == 1, "header name CRLF")?;
}

test("reject header value with crlf") {
    let names: Vec<string> = Vec::new();
    let values: Vec<string> = Vec::new();
    names.push("X-Trace");
    values.push("a\r\nb");
    assert(headers_have_crlf(names, values) == 1, "header value CRLF")?;
}

test("extras sanitize rejects injected header line") {
    let bad = "X-Trace: a\r\nb\r\n";
    let r = extras_sanitize(bad);
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "extras sanitize BadUrl")?;
}
