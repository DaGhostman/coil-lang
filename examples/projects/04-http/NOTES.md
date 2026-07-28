# 04-http — notes

## What it shows

Userland HTTP/1.1 client under `stdlib/http` (`get` / `post` / `request`) against
a local cleartext TCP server in the same project. HTTPS uses
`io::net::tls::connect` (verified; never insecure by default) — not exercised
in CI without a local cert.

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

- No redirects, cookies, pooling, timeouts, HTTP/2+
- No chunked transfer encoding
- `Connection: close` only
- Custom request headers are accepted by the API but not yet serialized into the wire head
- Prefer Result-mode helpers (`status_code` / `response_status`) over raw field
  access when crossing module/Result boundaries
