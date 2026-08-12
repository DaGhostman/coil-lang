// Pure unit tests: response parse (no sockets).
use string::{to_bytes};
use http::url::{find_bytes};
use http::response::{
    header_count,
    header_get,
    parse_response,
    response_body_len,
    response_status,
};

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
    let x: Vec<byte> = Vec::new();
    x.push(88);
    if find_bytes(r.body, x) != 0 { panic "first byte X"; }
}

test("reject truncated body shorter than content-length") {
    let raw = to_bytes("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nok");
    let r = parse_response(raw);
    assert(match r {
        Result::Ok(_) => false,
        Result::Err(_) => true,
    }, "expected BadResponse for truncated body")?;
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
