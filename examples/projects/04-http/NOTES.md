# 04-http — notes

## What it shows

Userland HTTP/1.1 client under `stdlib/http` (`get` / `post` / `request`) against
a local cleartext TCP server in the same project. HTTPS uses
`tcp::connect` + `io::net::tls::client::enable(..., { verify: true, ca_pem: "", timeout_ms: 0 })`
(never insecure by default) — not exercised in CI without a local cert.

## Run

```bash
./examples/projects/04-http/demo.sh
# ok
```

## Test

```bash
cd examples/projects/04-http && coil test
# or: ./examples/projects/run-tests.sh
```

## Layout

| File | Role |
|------|------|
| `stdlib/http/url.hy` | URL parse, `HttpError`, `Headers`, request build, response parse |
| `stdlib/http/request.hy` | Re-exports `http::url` (keeps the planned path; avoid multi-glob) |
| `stdlib/http/response.hy` | Re-exports `http::url` (same) |
| `stdlib/http/client.hy` | `get` / `post` / `request` (+ `status_code` / `body_len`) |
| `src/server.hy` | One-shot HTTP/1.1 responder (drains request head first) |
| `src/main.hy` | Client `get("http://127.0.0.1:41250/")` |

Impl detail: request/response helpers live in `url.hy` so `client` depends on a
**single** sibling module. Globbing `http::request` + `http::response` (each
`use http::url::*`) can hide `url` symbols.

## Limitations (v1)

- No redirects, cookies, pooling, caller-configured timeouts, HTTP/2+
- No chunked transfer encoding
- `Connection: close` only (caller Host / Content-Length / Connection are ignored
  so the client always emits correct values; other custom headers go on the wire)
- Prefer Result-mode helpers (`status_code` / `response_status`) over raw field
  access when crossing module/Result boundaries
- `http::client` imports `io::net::tls::client` — requires Cargo feature `tls` (default-on)
- IPv6 URL literals not supported (first `:` before `/` is treated as the port)
- Connect / TLS errors collapse to `HttpError::Io`; TLS host errors map to
  raw `IoError` variants only when using IO/TLS APIs directly
- The stdlib client currently requests no TCP connect or TLS handshake deadline
- CRLF (CR/LF) in URL host/path, method, or header names/values → `HttpError::BadUrl`
- `Content-Length` greater than available body bytes → `HttpError::BadResponse`

## Known compiler workaround

`body_len_str` / `cl_trailer` use length lookup tables instead of always calling
`int_to_dec` when concatenating into the request head under Result-mode deps —
`int_to_dec` has been flaky (SEGV) on some lengths. Track as a known compiler
issue; do not expand the workaround further unless needed.

Also:
- Do not `raise` / `?` inside `build_request_head*` (poisons Ok-path string concat).
- `to_bytes(s)` invalidates `s` for later use — CRLF scans on URL host/path use
  raw bytes before `bytes_to_string`; method injection is caught via
  `request_line_ok` on the built head; header injection via `extras_sanitize`.
- Query-only URLs (`http://host?q=`) do not get an automatic `/` prefix (same
  Result-mode concat SEGV) — write `/?q=` explicitly.