# 04-http — notes

Showcase for the [coil-http](https://github.com/ardax-corp/coil-http) package
(`Client` / `Server`) against a local cleartext TCP server in this project.

## Run

```bash
./examples/projects/04-http/demo.sh
# ok
```

## Test

```bash
cd examples/projects/04-http && coil test
```

Requires sibling checkouts: `../coil-http`, `../coil-stdlib` (for `conv` / `io::sync`).

## Layout

| File | Role |
|------|------|
| `src/server.hy` | One-shot fixture server (until class `Server` demo replaces it) |
| `src/main.hy` | `Client::get("http://127.0.0.1:41250/")` |

Package sources: [coil-http](https://github.com/ardax-corp/coil-http).
