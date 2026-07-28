// Pure unit tests: response parse (no sockets).
use io::*;
use http::url::*;

test("parse response with content-length") {
    let raw = to_bytes("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\n\r\nok");
    let r = match parse_response(raw) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(r.status == 200, "status")?;
    assert(len(r.body) == 2, "body len")?;
    assert(header_get(r, "Content-Type") == "text/plain", "content-type")?;
}

test("parse response read-to-close body") {
    let raw = to_bytes("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
    let r = match parse_response(raw) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(r.status == 204, "status")?;
    assert(len(r.body) == 0, "empty body")?;
}

test("response_status helper") {
    let raw = to_bytes("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    let r = match parse_response(raw) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    let st = match response_status(r) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "status helper",
    };
    let n = match response_body_len(r) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "body helper",
    };
    assert(st == 200, "status")?;
    assert(n == 2, "body len")?;
}

test("content-length header is case-insensitive") {
    let raw = to_bytes("HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nabcXXXX");
    let r = match parse_response(raw) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(r.status == 200, "status")?;
    assert(len(r.body) == 3, "truncated to content-length")?;
}

test("content-length truncates longer rest") {
    let raw = to_bytes("HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nXY");
    let r = match parse_response(raw) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(len(r.body) == 1, "body len")?;
    let x: [byte] = [];
    x[] = 88;
    if find_bytes(r.body, x) != 0 { panic "first byte X"; }
}

test("short body when content-length exceeds rest") {
    // Current v1 accepts truncated payloads as success (body = full rest).
    // Lock the behavior so a future BadResponse change is intentional.
    let raw = to_bytes("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nab");
    let r = match parse_response(raw) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(r.status == 200, "status")?;
    assert(len(r.body) == 2, "uses available rest")?;
}

test("reject response without header terminator") {
    let raw = to_bytes("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n");
    let r = parse_response(raw);
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected bad response Err")?;
}

test("header_get missing returns empty") {
    let raw = to_bytes("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    let r = match parse_response(raw) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(r.status == 404, "status")?;
    assert(header_get(r, "X-Missing") == "", "missing header")?;
}

test("parses multiple response headers") {
    let raw = to_bytes("HTTP/1.1 201 Created\r\nContent-Length: 0\r\nX-A: 1\r\nX-B: 2\r\n\r\n");
    let r = match parse_response(raw) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "parse failed",
    };
    assert(r.status == 201, "status")?;
    assert(header_get(r, "X-A") == "1", "x-a")?;
    assert(header_get(r, "X-B") == "2", "x-b")?;
    assert(header_count(r) >= 3, "header count")?;
}
