// URL parse for the HTTP client (byte-scan; no string slice/index).
use io::*;

enum HttpError {
    BadUrl,
    BadResponse,
    UnsupportedScheme,
    Io,
}

class Url {
    scheme: string,
    host: string,
    port: int,
    path: string,
}

class Headers {
    names: [string],
    values: [string],
}

fn empty_headers() -> Headers {
    let names: [string] = [];
    let values: [string] = [];
    return new Headers(names, values);
}

fn header_add(Headers h, string name, string value) {
    h.names[] = name;
    h.values[] = value;
    return h;
}

fn url_scheme(Url u) -> Result<string, HttpError> {
    return u.scheme;
}

fn url_host(Url u) -> Result<string, HttpError> {
    return u.host;
}

fn url_port(Url u) -> Result<int, HttpError> {
    return u.port;
}

fn url_path(Url u) -> Result<string, HttpError> {
    return u.path;
}

fn headers_count(Headers h) -> Result<int, HttpError> {
    return len(h.names);
}

fn header_name_at(Headers h, int i) -> Result<string, HttpError> {
    return h.names[i];
}

fn header_value_at(Headers h, int i) -> Result<string, HttpError> {
    return h.values[i];
}


fn bytes_slice([byte] src, int start, int end) -> [byte] {
    let out: [byte] = [];
    let i = start;
    while i < end {
        if i < len(src) {
            out[] = src[i];
        }
        i = i + 1;
    }
    return out;
}

fn bytes_to_string([byte] b) -> Result<string, HttpError> {
    return match from_bytes(b) {
        Result::Ok(s) => s,
        Result::Err(_) => raise HttpError::BadUrl,
    };
}

fn find_bytes([byte] hay, [byte] needle) -> int {
    let hn = len(hay);
    let nn = len(needle);
    if nn == 0 {
        return 0;
    }
    if nn > hn {
        return 999999;
    }
    let i = 0;
    while i + nn <= hn {
        let ok = true;
        let j = 0;
        while j < nn {
            if hay[i + j] != needle[j] {
                ok = false;
            }
            j = j + 1;
        }
        if ok {
            return i;
        }
        i = i + 1;
    }
    return 999999;
}

fn parse_port_digits([byte] b, int start, int end) -> int {
    let n = 0;
    let i = start;
    let zero: byte = 48;
    let nine: byte = 57;
    if start >= end {
        return 999999;
    }
    while i < end {
        let c = b[i];
        if c < zero {
            return 999999;
        }
        if c > nine {
            return 999999;
        }
        n = n * 10 + ((c as int) - 48);
        i = i + 1;
    }
    return n;
}

fn http_err_bad_url() -> Result<(), HttpError> {
    raise HttpError::BadUrl;
}

fn http_err_bad_response() -> Result<(), HttpError> {
    raise HttpError::BadResponse;
}

// Reject CR (13) / LF (10) so path/host/method/headers cannot inject request lines.
// Plain int return — keep raise/`?` out of build_request_head* (concat poison).
fn bytes_have_crlf([byte] b) -> int {
    let i = 0;
    let cr: byte = 13;
    let lf: byte = 10;
    while i < len(b) {
        if b[i] == cr {
            return 1;
        }
        if b[i] == lf {
            return 1;
        }
        i = i + 1;
    }
    return 0;
}

fn has_crlf(string s) -> int {
    return bytes_have_crlf(to_bytes(s));
}

// `to_bytes` invalidates the input string (VM quirk); rebuild after a CRLF scan.
fn str_reject_crlf(string s) -> Result<string, HttpError> {
    let b = to_bytes(s);
    if bytes_have_crlf(b) == 1 {
        http_err_bad_url()?;
    }
    return match from_bytes(b) {
        Result::Ok(s2) => s2,
        Result::Err(_) => {
            http_err_bad_url()?;
            ""
        },
    };
}

// True (1) if the first header line of an HTTP message contains "HTTP/1.1"
// (rejects method/path CRLF injection, which truncates the request line).
fn request_line_ok([byte] head) -> int {
    let cr: byte = 13;
    let lf: byte = 10;
    let i = 0;
    let n = len(head);
    while i + 1 < n {
        if head[i] == cr {
            if head[i + 1] == lf {
                let needle: [byte] = [72, 84, 84, 80, 47, 49, 46, 49];
                let j = 0;
                while j + 8 <= i {
                    let ok = true;
                    let k = 0;
                    while k < 8 {
                        if head[j + k] != needle[k] {
                            ok = false;
                        }
                        k = k + 1;
                    }
                    if ok {
                        return 1;
                    }
                    j = j + 1;
                }
                return 0;
            }
        }
        i = i + 1;
    }
    return 0;
}

// After format_extra_headers_str: every non-empty line must contain ':'.
// Rebuilds extras (to_bytes invalidates the input string).
fn extras_sanitize(string extras) -> Result<string, HttpError> {
    let b = to_bytes(extras);
    let cr: byte = 13;
    let lf: byte = 10;
    let colon: byte = 58;
    let line_start = 0;
    let i = 0;
    let n = len(b);
    while i + 1 < n {
        if b[i] == cr {
            if b[i + 1] == lf {
                if i > line_start {
                    let has_colon = 0;
                    let j = line_start;
                    while j < i {
                        if b[j] == colon {
                            has_colon = 1;
                        }
                        j = j + 1;
                    }
                    if has_colon == 0 {
                        http_err_bad_url()?;
                    }
                }
                line_start = i + 2;
                i = i + 2;
            } else {
                i = i + 1;
            }
        } else {
            i = i + 1;
        }
    }
    return match from_bytes(b) {
        Result::Ok(s) => s,
        Result::Err(_) => {
            http_err_bad_url()?;
            ""
        },
    };
}

fn headers_have_crlf([string] names, [string] values) -> int {
    let i = 0;
    let n = len(names);
    while i < n {
        if has_crlf(names[i]) == 1 {
            return 1;
        }
        if has_crlf(values[i]) == 1 {
            return 1;
        }
        i = i + 1;
    }
    return 0;
}

fn http_err_unsupported_scheme() -> Result<(), HttpError> {
    raise HttpError::UnsupportedScheme;
}

fn http_err_io() -> Result<(), HttpError> {
    raise HttpError::Io;
}

fn http_fail_stream() -> Result<Stream, HttpError> {
    raise HttpError::Io;
}

fn http_fail_bytes() -> Result<[byte], HttpError> {
    raise HttpError::Io;
}

fn http_fail_unit() -> Result<(), HttpError> {
    raise HttpError::Io;
}

fn parse_url(string s) -> Result<Url, HttpError> {
    let b = to_bytes(s);
    let sep: [byte] = [58, 47, 47]; // ://
    let sep_at = find_bytes(b, sep);
    if sep_at == 999999 {
        http_err_bad_url()?;
    }
    let scheme_b = bytes_slice(b, 0, sep_at);
    let scheme = bytes_to_string(scheme_b)?;
    let rest_start = sep_at + 3;
    if rest_start > len(b) {
        http_err_bad_url()?;
    }

    let slash: byte = 47;
    let qmark: byte = 63;
    let colon: byte = 58;

    let i = rest_start;
    let host_end = len(b);
    let path_start = len(b);
    let port_start = 0;
    let has_port = 0;
    let found_path = 0;

    while i < len(b) {
        let c = b[i];
        if found_path == 0 {
            if c == colon {
                if has_port == 0 {
                    host_end = i;
                    port_start = i + 1;
                    has_port = 1;
                }
            }
            if c == slash {
                if has_port == 0 {
                    host_end = i;
                }
                path_start = i;
                found_path = 1;
            }
            if c == qmark {
                if has_port == 0 {
                    host_end = i;
                }
                path_start = i;
                found_path = 1;
            }
        }
        i = i + 1;
    }

    if found_path == 0 {
        if has_port == 1 {
            host_end = port_start - 1;
        } else {
            host_end = len(b);
        }
        path_start = len(b);
    } else {
        if has_port == 1 {
            if port_start >= path_start {
                has_port = 0;
            }
        }
    }

    if host_end <= rest_start {
        http_err_bad_url()?;
    }
    let host_b = bytes_slice(b, rest_start, host_end);
    if bytes_have_crlf(host_b) == 1 {
        http_err_bad_url()?;
    }
    let host = bytes_to_string(host_b)?;

    let port = 80;
    if scheme == "https" {
        port = 443;
    }
    if scheme == "http" {
        port = 80;
    }
    if has_port == 1 {
        let port_end = path_start;
        if found_path == 0 {
            port_end = len(b);
        }
        let parsed = parse_port_digits(b, port_start, port_end);
        if parsed == 999999 {
            http_err_bad_url()?;
        }
        port = parsed;
    }

    let path = "/";
    if found_path == 1 {
        if b[path_start] == qmark {
            // Bare `host?q=` → path "?q=…". Prefer `/?q=` in URLs: prefixing
            // `"/" + q` here under Result mode hits a known compiler SEGV.
            let path_b = bytes_slice(b, path_start, len(b));
            if bytes_have_crlf(path_b) == 1 {
                http_err_bad_url()?;
            }
            path = bytes_to_string(path_b)?;
        } else {
            let path_b = bytes_slice(b, path_start, len(b));
            if bytes_have_crlf(path_b) == 1 {
                http_err_bad_url()?;
            }
            path = bytes_to_string(path_b)?;
        }
    }

    if scheme == "http" {
        return new Url(scheme, host, port, path);
    }
    if scheme == "https" {
        return new Url(scheme, host, port, path);
    }
    http_err_unsupported_scheme()?;
    return new Url(scheme, host, port, path);
}


// --- request builders (kept in url for single-import client graph) ---
// Serialize an HTTP/1.1 request message to `[byte]`.

fn digit_char(int d) -> string {
    if d == 0 { return "0"; }
    if d == 1 { return "1"; }
    if d == 2 { return "2"; }
    if d == 3 { return "3"; }
    if d == 4 { return "4"; }
    if d == 5 { return "5"; }
    if d == 6 { return "6"; }
    if d == 7 { return "7"; }
    if d == 8 { return "8"; }
    return "9";
}

// Decimal int → string without `format` (avoids Result-mode SEGV in deps).
// Only non-negative lengths/ports are expected.
fn int_to_dec(int n) -> string {
    if n == 0 {
        return "0";
    }
    let x = n;
    let rev = "";
    while x > 0 {
        let d = x % 10;
        rev = digit_char(d) + rev;
        x = x / 10;
    }
    return rev;
}

fn host_header_value(Url u) -> Result<string, HttpError> {
    let host = url_host(u)?;
    let port = url_port(u)?;
    let scheme = url_scheme(u)?;
    if scheme == "http" {
        if port == 80 {
            return host;
        }
    }
    if scheme == "https" {
        if port == 443 {
            return host;
        }
    }
    return host + ":" + int_to_dec(port);
}

fn body_len_str(int body_len) -> string {
    // Prefer a lookup over `int_to_dec` here — concatenating
    // `int_to_dec(body_len)` into the request head in Result-mode
    // dependency helpers has been flaky (SEGV) on some lengths.
    if body_len == 0 { return "0"; }
    if body_len == 1 { return "1"; }
    if body_len == 2 { return "2"; }
    if body_len == 3 { return "3"; }
    if body_len == 4 { return "4"; }
    if body_len == 5 { return "5"; }
    if body_len == 6 { return "6"; }
    if body_len == 7 { return "7"; }
    if body_len == 8 { return "8"; }
    if body_len == 9 { return "9"; }
    if body_len == 10 { return "10"; }
    if body_len == 11 { return "11"; }
    if body_len == 12 { return "12"; }
    if body_len == 13 { return "13"; }
    if body_len == 14 { return "14"; }
    if body_len == 15 { return "15"; }
    if body_len == 16 { return "16"; }
    if body_len == 17 { return "17"; }
    if body_len == 32 { return "32"; }
    if body_len == 64 { return "64"; }
    if body_len == 128 { return "128"; }
    if body_len == 256 { return "256"; }
    if body_len == 512 { return "512"; }
    if body_len == 1024 { return "1024"; }
    return int_to_dec(body_len);
}

fn concat_bytes([byte] a, [byte] b) -> [byte] {
    let out: [byte] = [];
    let i = 0;
    while i < len(a) {
        out[] = a[i];
        i = i + 1;
    }
    let j = 0;
    while j < len(b) {
        out[] = b[j];
        j = j + 1;
    }
    return out;
}

fn cl_trailer(int body_len) -> string {
    if body_len == 0 {
        return "0\r\nConnection: close\r\n\r\n";
    } else if body_len == 1 {
        return "1\r\nConnection: close\r\n\r\n";
    } else if body_len == 2 {
        return "2\r\nConnection: close\r\n\r\n";
    } else if body_len == 3 {
        return "3\r\nConnection: close\r\n\r\n";
    } else if body_len == 4 {
        return "4\r\nConnection: close\r\n\r\n";
    } else if body_len == 5 {
        return "5\r\nConnection: close\r\n\r\n";
    } else if body_len == 6 {
        return "6\r\nConnection: close\r\n\r\n";
    } else if body_len == 7 {
        return "7\r\nConnection: close\r\n\r\n";
    } else if body_len == 8 {
        return "8\r\nConnection: close\r\n\r\n";
    } else if body_len == 9 {
        return "9\r\nConnection: close\r\n\r\n";
    } else if body_len == 10 {
        return "10\r\nConnection: close\r\n\r\n";
    } else if body_len == 16 {
        return "16\r\nConnection: close\r\n\r\n";
    } else if body_len == 17 {
        return "17\r\nConnection: close\r\n\r\n";
    } else if body_len == 32 {
        return "32\r\nConnection: close\r\n\r\n";
    } else if body_len == 64 {
        return "64\r\nConnection: close\r\n\r\n";
    } else if body_len == 128 {
        return "128\r\nConnection: close\r\n\r\n";
    } else if body_len == 256 {
        return "256\r\nConnection: close\r\n\r\n";
    } else if body_len == 512 {
        return "512\r\nConnection: close\r\n\r\n";
    } else if body_len == 1024 {
        return "1024\r\nConnection: close\r\n\r\n";
    } else {
        return body_len_str(body_len) + "\r\nConnection: close\r\n\r\n";
    }
}

fn header_name_eq_ci(string a, string b) -> int {
    let ab = to_bytes(a);
    let bb = to_bytes(b);
    if len(ab) != len(bb) {
        return 0;
    }
    let i = 0;
    while i < len(ab) {
        let x = ab[i] as int;
        let y = bb[i] as int;
        if x >= 65 {
            if x <= 90 {
                x = x + 32;
            }
        }
        if y >= 65 {
            if y <= 90 {
                y = y + 32;
            }
        }
        if x != y {
            return 0;
        }
        i = i + 1;
    }
    return 1;
}

// Host / Content-Length / Connection are always emitted by the client;
// format_extra_headers_str skips those names (common ASCII case spellings).
// Callers must reject header name/value CRLF via headers_have_crlf before
// formatting; formatting itself stays raise-free and to_bytes-light.
// Exact `==` only — do not call header_name_eq_ci / to_bytes on live names
// here (to_bytes invalidates string slots in the headers arrays).

fn is_reserved_request_header(string name) -> int {
    if name == "Host" { return 1; }
    if name == "host" { return 1; }
    if name == "HOST" { return 1; }
    if name == "Content-Length" { return 1; }
    if name == "content-length" { return 1; }
    if name == "CONTENT-LENGTH" { return 1; }
    if name == "Content-length" { return 1; }
    if name == "Connection" { return 1; }
    if name == "connection" { return 1; }
    if name == "CONNECTION" { return 1; }
    return 0;
}

fn format_extra_headers_str([string] names, [string] values) -> string {
    // Precondition: caller ensures there is at least one non-reserved header.
    let first = 999999;
    let i = 0;
    let n = len(names);
    while i < n {
        if is_reserved_request_header(names[i]) == 0 {
            if first == 999999 {
                first = i;
            }
        }
        i = i + 1;
    }
    if first == 999999 {
        return "__NONE__";
    }
    let acc = names[first] + ": " + values[first] + "\r\n";
    let j = first + 1;
    while j < n {
        if is_reserved_request_header(names[j]) == 0 {
            acc = acc + names[j] + ": " + values[j] + "\r\n";
        }
        j = j + 1;
    }
    return acc;
}

fn format_extra_headers(Headers headers) -> [byte] {
    let s = format_extra_headers_str(headers.names, headers.values);
    if s == "__NONE__" {
        let empty: [byte] = [];
        return empty;
    }
    return to_bytes(s);
}

fn build_request_head(string method, Url u, Headers headers, int body_len) -> Result<[byte], HttpError> {
    // Inline Host construction (avoid nested Result `?` through host_header_value).
    // No raise/`?` here — poisons Ok-path string concat. Callers use has_crlf / parse_url.
    let host = u.host;
    let port = u.port;
    let scheme = u.scheme;
    let path = u.path;
    let host_hdr = host;
    if scheme == "http" {
        if port != 80 {
            host_hdr = host + ":" + int_to_dec(port);
        }
    } else {
        if scheme == "https" {
            if port != 443 {
                host_hdr = host + ":" + int_to_dec(port);
            }
        } else {
            host_hdr = host + ":" + int_to_dec(port);
        }
    }
    let prefix = method + " " + path + " HTTP/1.1\r\nHost: " + host_hdr + "\r\nContent-Length: ";
    let rest = cl_trailer(body_len);
    return concat_bytes(to_bytes(prefix), to_bytes(rest));
}

fn build_request_head_extras(string method, Url u, string extras, int body_len) -> Result<[byte], HttpError> {
    // Inserts non-empty `extras` ("Name: value\r\n" lines) after Host.
    // Caller must not pass the "__NONE__" / "__CRLF__" sentinels.
    // Header CRLF is rejected in format_extra_headers_str. No raise/`?` here.
    let host = u.host;
    let port = u.port;
    let scheme = u.scheme;
    let path = u.path;
    let host_hdr = host;
    if scheme == "http" {
        if port != 80 {
            host_hdr = host + ":" + int_to_dec(port);
        }
    } else {
        if scheme == "https" {
            if port != 443 {
                host_hdr = host + ":" + int_to_dec(port);
            }
        } else {
            host_hdr = host + ":" + int_to_dec(port);
        }
    }
    let prefix = method + " " + path + " HTTP/1.1\r\nHost: " + host_hdr + "\r\n" + extras + "Content-Length: ";
    let rest = cl_trailer(body_len);
    return concat_bytes(to_bytes(prefix), to_bytes(rest));
}

fn req_parse_url(string s) -> Result<Url, HttpError> {
    return parse_url(s)?;
}

fn req_empty_headers() -> Headers {
    return empty_headers();
}

// --- response parse (kept in url for single-import client graph) ---
// Response parse for HTTP/1.1 (byte-scan).

class Response {
    status: int,
    header_names: [string],
    header_values: [string],
    body: [byte],
}

fn make_response(int status, [string] names, [string] values, [byte] body) -> Response {
    return new Response(status, names, values, body);
}

fn response_status(Response r) -> Result<int, HttpError> {
    return r.status;
}

fn response_body_len(Response r) -> Result<int, HttpError> {
    return len(r.body);
}

fn header_count(Response r) -> int {
    return len(r.header_names);
}

fn header_name_at_resp(Response r, int i) -> string {
    return r.header_names[i];
}

fn header_get(Response r, string name) -> string {
    let i = 0;
    let n = len(r.header_names);
    while i < n {
        if r.header_names[i] == name {
            return r.header_values[i];
        }
        i = i + 1;
    }
    return "";
}

fn find_header_end([byte] buf) -> int {
    let cr: byte = 13;
    let lf: byte = 10;
    let i = 0;
    let n = len(buf);
    while i + 3 < n {
        if buf[i] == cr {
            if buf[i + 1] == lf {
                if buf[i + 2] == cr {
                    if buf[i + 3] == lf {
                        return i;
                    }
                }
            }
        }
        i = i + 1;
    }
    return 999999;
}

fn bytes_slice_resp([byte] src, int start, int end) -> [byte] {
    let out: [byte] = [];
    let i = start;
    while i < end {
        if i < len(src) {
            out[] = src[i];
        }
        i = i + 1;
    }
    return out;
}

fn find_crlf([byte] buf, int from) -> int {
    let cr: byte = 13;
    let lf: byte = 10;
    let i = from;
    while i + 1 < len(buf) {
        if buf[i] == cr {
            if buf[i + 1] == lf {
                return i;
            }
        }
        i = i + 1;
    }
    return 999999;
}

fn parse_status_code([byte] line) -> Result<int, HttpError> {
    let sp: byte = 32;
    let i = 0;
    let n = len(line);
    while i < n {
        if line[i] == sp {
            i = i + 1;
            break;
        }
        i = i + 1;
    }
    let start = i;
    while i < n {
        if line[i] == sp {
            break;
        }
        i = i + 1;
    }
    if i <= start {
        http_err_bad_response()?;
    }
    let code = 0;
    let j = start;
    while j < i {
        code = code * 10 + ((line[j] as int) - 48);
        j = j + 1;
    }
    return code;
}

fn parse_int_bytes([byte] b) -> Result<int, HttpError> {
    let n = len(b);
    if n == 0 {
        http_err_bad_response()?;
    }
    let v = 0;
    let i = 0;
    while i < n {
        v = v * 10 + ((b[i] as int) - 48);
        i = i + 1;
    }
    return v;
}

fn find_byte([byte] line, byte needle) -> int {
    let k = 0;
    while k < len(line) {
        if line[k] == needle {
            return k;
        }
        k = k + 1;
    }
    return 999999;
}

fn parse_status_line([byte] header_bytes) -> Result<int, HttpError> {
    let eol = find_crlf(header_bytes, 0);
    let line_end = len(header_bytes);
    if eol != 999999 {
        line_end = eol;
    }
    let line = bytes_slice_resp(header_bytes, 0, line_end);
    return parse_status_code(line)?;
}

fn append_header_line([string] names, [string] values, [byte] line) -> Result<int, HttpError> {
    let colon: byte = 58;
    let sp: byte = 32;
    let cpos = find_byte(line, colon);
    if cpos == 999999 {
        http_err_bad_response()?;
    }
    let name_b = bytes_slice_resp(line, 0, cpos);
    let val_start = cpos + 1;
    if val_start < len(line) {
        if line[val_start] == sp {
            val_start = val_start + 1;
        }
    }
    let val_b = bytes_slice_resp(line, val_start, len(line));
    let name = bytes_to_string(name_b)?;
    let value = bytes_to_string(val_b)?;
    names[] = name;
    values[] = value;
    return 0;
}

fn content_length_from([string] names, [string] values) -> Result<int, HttpError> {
    let i = 0;
    while i < len(names) {
        if header_name_eq_ci(names[i], "Content-Length") == 1 {
            return parse_int_bytes(to_bytes(values[i]))?;
        }
        i = i + 1;
    }
    return 999999;
}

fn parse_response([byte] raw) -> Result<Response, HttpError> {
    let sep = find_header_end(raw);
    if sep == 999999 {
        http_err_bad_response()?;
    }
    let header_bytes = bytes_slice_resp(raw, 0, sep);
    let rest = bytes_slice_resp(raw, sep + 4, len(raw));

    let status = parse_status_line(header_bytes)?;

    let names: [string] = [];
    let values: [string] = [];
    let eol0 = find_crlf(header_bytes, 0);
    let pos = 0;
    if eol0 != 999999 {
        pos = eol0 + 2;
    }
    let n = len(header_bytes);
    while pos < n {
        let eol = find_crlf(header_bytes, pos);
        let line_end = n;
        if eol != 999999 {
            line_end = eol;
        }
        if line_end > pos {
            let line = bytes_slice_resp(header_bytes, pos, line_end);
            append_header_line(names, values, line)?;
        }
        if eol == 999999 {
            pos = n;
        } else {
            pos = eol + 2;
        }
    }

    let content_length = content_length_from(names, values)?;
    let body = rest;
    if content_length != 999999 {
        if content_length > len(rest) {
            raise HttpError::BadResponse;
        }
        if content_length < len(rest) {
            body = bytes_slice_resp(rest, 0, content_length);
        }
    }
    return make_response(status, names, values, body);
}