# HTTP client + TLS stdlib design

**Date:** 2026-07-28  
**Status:** Approved for implementation (three PRs)

## Goal

Coil programs can open TLS streams from the host and call a userland HTTP/1.1 client under `stdlib/`, without implementing TLS, HTTP/2, HTTP/3, or QUIC in coil.

## Architecture

- **Transport (VM):** rustls behind `io::net::tls::{client,server}`, returning the same opaque `Stream` as TCP/files.
- **Application (coil):** HTTP/1.1 request/response framing in `stdlib/http/*`.
- **Prerequisite:** Multi-module IO HostInvoke must work so library modules can own sockets.

```text
coil http::client
    │
    ├─ http  → io::net::tcp::connect
    └─ https → io::net::tcp::connect
              └─ io::net::tls::client::enable(s, host, { verify: true })
              └─ Stream (read/write/close)
```

## Delivery

| PR | Scope |
|----|--------|
| 1 | Verify multi-module IO HostInvoke + `?` (pool retention already on main); regression golden; update NOTES |
| 2 | Host `io::net::tls::{client,server}::{enable, disable}` via rustls |
| 3 | Coil `stdlib/http` request builder (`get` / `post` / `request`) |

Land serially. Parallelize investigation only.

## Locked decisions

- TLS handshake stays in the host (rustls); coil never implements the handshake.
- Certs v1: webpki-roots (Mozilla CA set) + `enable(..., { verify: false })` for local/dev.
  (OS/native trust store deferred; portable default for embedders/CI.)
- Custom PEM CA / private roots: deferred.
- HTTP v1: request builder; HTTP/1.1 only; `Connection: close`.
- No redirects, cookies, pooling, timeouts API, ALPN, HTTP/2+.

## PR 1 — Multi-module IO

**Problem:** Dependency modules calling IO + `?` historically failed when the constant pool was cleared between `compile_module` calls, orphaning `JumpIfMatch` indices.

**Done when:** A dependency can call IO HostInvoke + `?`; `multi_file_io_hostinvoke_try_in_dependency` (or equivalent) exists; project NOTES no longer require entry-file-only IO.

## PR 2 — `io::net::tls`

```coil
use io::net::tcp::*;
use io::net::tls::client::*;
let s = connect("example.com", 443)?;
let s = enable(s, "example.com", { verify: true })?;
let s = enable(s, "127.0.0.1", { verify: false })?;  // local/dev
let s = disable(s)?;
```

- Same `Stream` APIs: `read` / `write` / `write_all` / `read_exact` / `read_to_end` / `close`.
- Cargo feature `tls` (default-on), like `crypto`.
- Out of scope (HTTP v1): PEM CA on the client, ALPN, client certs.
  Server TLS is `io::net::tls::server::{enable,disable}` (PEM cert/key opts),
  separate from the HTTP client path.

## PR 3 — `stdlib/http`

```text
stdlib/http/url.hy
stdlib/http/request.hy
stdlib/http/response.hy
stdlib/http/client.hy
```

- Wire `./stdlib` in `coil.toml` `[module].roots`.
- `get` / `post` / `request` → `Result<Response, …>` with status, headers, body (`[byte]`).
- `http` → TCP; `https` → `tcp::connect` + `tls::client::enable(..., { verify: true })` (never insecure by default).

## Explicit non-goals

- TLS/HTTP2/HTTP3/QUIC in coil
- DIY TLS from virtual `crypto`
- Custom PEM / client certs / TLS listen (v1)
- Keep-alive, redirects, cookies, timeouts API
