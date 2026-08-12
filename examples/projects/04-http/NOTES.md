# 04-http — notes

Showcase for the [coil-stdlib HTTP client](https://github.com/ardax-corp/coil-stdlib/blob/main/docs/http.md)
(`get` / `post` / `request`) against a local cleartext TCP server in this
project. HTTPS uses virtual `io::net::tls` (not exercised in CI without a local cert).

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
| `src/server.hy` | One-shot HTTP/1.1 responder (drains request head first) |
| `src/main.hy` | Client `get("http://127.0.0.1:41250/")` |

Client sources live in [coil-stdlib](https://github.com/ardax-corp/coil-stdlib) (`src/http/*.hy`).
API, limitations, and compiler workarounds: [coil-stdlib HTTP](https://github.com/ardax-corp/coil-stdlib/blob/main/docs/http.md).
