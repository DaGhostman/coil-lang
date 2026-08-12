# HTTP client and server

HTTP is **userland** in [coil-http](https://github.com/ardax-corp/coil-http)
(`ardax-corp/coil-http`), not a compiler builtin.

**Docs:** [coil-http](https://github.com/ardax-corp/coil-http/blob/main/docs/README.md)

Add `../coil-http/src` (or `spool` install) to `[module].roots`. Showcase:
[`examples/projects/04-http`](../../examples/projects/04-http).

```coil
use http::{Client, Server, Request, Response};

fn main() {
    let client = Client::new();
    let r = client.get("http://127.0.0.1:8080/")?;
}
```

Legacy function-oriented client lived in [coil-stdlib](https://github.com/ardax-corp/coil-stdlib)
until v0.2; use coil-http for new projects.
