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
