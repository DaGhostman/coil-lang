# HTTP/1.1 client (`stdlib/http`)

Coil ships a small **userland** HTTP/1.1 request builder under `stdlib/http/`.
It speaks cleartext TCP (`io::net::tcp::connect`) for `http://` and verified TLS
(`tcp::connect` + `io::net::tls::client::enable(..., { verify: true, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 })`)
for `https://` — never insecure by default.

## Setup

Add the stdlib root to your project `coil.toml`:

```toml
[module]
roots = ["./src", "./stdlib"]   # or point at the coil checkout's stdlib/
```

The workspace manifest already includes `./stdlib`. See
[`coil.toml.example`](../../coil.toml.example) and
[project config](../references/project-config.md).

## API

```coil
use http::client::*;

fn main() {
    match get("http://127.0.0.1:41250/") {
        Result::Ok(_) => { /* success */ },
        Result::Err(_) => { panic "get failed"; },
    };
}
```

| Function | Role |
|----------|------|
| `get(url)` | `GET` with empty body |
| `post(url, body)` | `POST` with `[byte]` body |
| `request(method, url, headers, body)` | Full builder |
| `status_code(r)` / `body_len(r)` | Result-mode accessors for `Response` |

`Response` carries `status: int`, parallel `header_names` / `header_values`, and
`body: [byte]`. Errors use `HttpError` (`BadUrl`, `BadResponse`,
`UnsupportedScheme`, `Io`).

Requests are HTTP/1.1 with `Host`, `Content-Length`, and `Connection: close`.
Extra headers passed to `request` are written on the wire; attempts to override
`Host` / `Content-Length` / `Connection` (common ASCII spellings such as
`host` / `HOST` / `content-length` / `CONTENT-LENGTH`) are ignored so the
client always emits those itself. Full Unicode/case-fold matching is out of
scope for v1 (`to_bytes` would invalidate live header name slots).

## Example

Self-contained cleartext demo (local server + client):

```bash
./examples/projects/04-http/demo.sh
# ok
```

Unit tests (URL / request / response parse, no network):

```bash
cd examples/projects/04-http && coil test
```

The HTTPS path uses webpki roots and no handshake deadline by default:

```coil
tls_enable(s, host, { verify: true, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 })
```

## Limitations (v1)

- No redirects, cookies, or connection pooling
- No chunked transfer encoding (uses `Content-Length` or read-to-close)
- No HTTP/2 / HTTP/3
- `http::client` requires Cargo feature `tls` (imports `io::net::tls::client`; default-on)
- IPv6 URL literals are not supported — the first `:` before `/` is the port
- HTTPS URLs should use a DNS hostname for SNI / cert name checks; literal-IP
  hosts may fail verification depending on the peer certificate
- Connect failures and TLS errors collapse to `HttpError::Io`; inspect the
  underlying `IoError` only when using raw IO/TLS APIs directly
- The stdlib client currently requests no TCP connect or TLS handshake deadline
- CR/LF in URL host/path, method, or header names/values → `HttpError::BadUrl`
- When `Content-Length` exceeds available body bytes → `HttpError::BadResponse`
- HTTPS against public hosts needs a normal PKI trust path; local MITM/dev
  certs need raw `tls::client::enable` with `ca_pem` / `ca_path`
  (`Option::Some(...)` appends to webpki) or `verify: false`; this client
  always verifies with webpki roots (no extras)

### Known compiler note

Request Content-Length formatting uses `body_len_str` / `cl_trailer` lookup
helpers because concatenating `int_to_dec` into the request head under
Result-mode dependency helpers has been flaky (SEGV) on some lengths. This is
a known compiler issue to track — not an HTTP design choice. Prefer `/?q=` over
bare `host?q=` (slash-prefix under Result-mode parse also SEGVs).
