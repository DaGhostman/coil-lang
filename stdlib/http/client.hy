// HTTP/1.1 client: get / post / request.
// Depends only on `http::url` (request/response impls live there) to avoid
// multi-glob import bugs across sibling http::* modules.
use io::{Stream, close};
use io::net::tcp::connect as tcp_connect;
use io::net::tls::client::enable as tls_enable;
use io::sync::{read_to_end, write_all};

use http::url::{
    Headers,
    HttpError,
    Response,
    Url,
    build_request_head,
    build_request_head_extras,
    concat_bytes,
    empty_headers,
    extras_sanitize,
    format_extra_headers_str,
    headers_have_crlf,
    http_err_bad_url,
    http_err_unsupported_scheme,
    http_fail_bytes,
    http_fail_stream,
    http_fail_unit,
    parse_response,
    parse_url,
    request_line_ok,
    response_body_len,
    response_status,
    url_host,
    url_port,
    url_scheme,
};

fn open_stream(Url u) -> Result<Stream, HttpError> {
    let scheme = url_scheme(u)?;
    let host = url_host(u)?;
    let port = url_port(u)?;
    if scheme == "http" {
        return match tcp_connect(host, port) {
            Result::Ok(s) => s,
            Result::Err(_) => http_fail_stream()?,
        };
    }
    if scheme == "https" {
        // Verified TLS only. For local/dev certs use tls::client::enable(...,
        // { verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 }) directly — this client
        // never skips trust checks.
        let s = match tcp_connect(host, port) {
            Result::Ok(s) => s,
            Result::Err(_) => http_fail_stream()?,
        };
        return match tls_enable(s, host, { verify: true, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 }) {
            Result::Ok(s) => s,
            Result::Err(_) => {
                match close(s) {
                    Result::Ok(_) => 0,
                    Result::Err(_) => 0,
                };
                http_fail_stream()?
            },
        };
    }
    http_err_unsupported_scheme()?;
    return http_fail_stream()?;
}

fn request_send(Vec<byte> head, Url u, Vec<byte> body) -> Result<Response, HttpError> {
    let msg = concat_bytes(head, body);
    let s = open_stream(u)?;
    match write_all(s, msg) {
        Result::Ok(_) => 0,
        Result::Err(_) => {
            http_fail_unit()?;
            0
        },
    };
    let raw = match read_to_end(s) {
        Result::Ok(b) => b,
        Result::Err(_) => http_fail_bytes()?,
    };
    match close(s) {
        Result::Ok(_) => 0,
        Result::Err(_) => 0,
    };
    return parse_response(raw)?;
}

/// Send arbitrary HTTP/1.1 request with custom method, headers, and body.
fn request(string method, string url, Headers headers, Vec<byte> body) -> Result<Response, HttpError> {
    let u = parse_url(url)?;
    let bl = len(body);
    let n = len(headers.names);
    if n > 0 {
        if headers_have_crlf(headers.names, headers.values) == 1 {
            http_err_bad_url()?;
        }
        let extras = format_extra_headers_str(headers.names, headers.values);
        if extras != "__NONE__" {
            let extras = extras_sanitize(extras)?;
            let head = build_request_head_extras(method, u, extras, bl)?;
            if request_line_ok(head) == 0 {
                http_err_bad_url()?;
            }
            return request_send(head, u, body)?;
        }
    }
    let head = build_request_head(method, u, headers, bl)?;
    if request_line_ok(head) == 0 {
        http_err_bad_url()?;
    }
    return request_send(head, u, body)?;
}

/// HTTP GET `url`; returns parsed `Response` or `HttpError`.
fn get(string url) -> Result<Response, HttpError> {
    let hs = empty_headers();
    let body: Vec<byte> = Vec::new();
    return request("GET", url, hs, body)?;
}

/// HTTP POST `url` with `body`; returns `Response` or `HttpError`.
fn post(string url, Vec<byte> body) -> Result<Response, HttpError> {
    let hs = empty_headers();
    return request("POST", url, hs, body)?;
}

/// HTTP status code from `Response`.
fn status_code(Response r) -> Result<int, HttpError> {
    return response_status(r)?;
}

/// Byte length of response body.
fn body_len(Response r) -> Result<int, HttpError> {
    return response_body_len(r)?;
}
