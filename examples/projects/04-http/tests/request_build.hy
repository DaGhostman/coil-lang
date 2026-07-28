use io::*;
use http::url::*;

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
    hs = header_add(hs, "X-Trace", "abc");
    hs = header_add(hs, "Accept", "text/plain");
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
    let body: [byte] = [];
    body[] = 65;
    body[] = 66;
    let msg = concat_bytes(head, body);
    if len(msg) != len(head) + 2 { panic "concat len"; }
}
