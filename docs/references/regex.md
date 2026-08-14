# Regular expressions

PCRE2 regex is **userland** in [coil-regex](https://github.com/ardax-corp/coil-regex), not a compiler builtin.

## Install via spool (future)

```toml
[dependencies]
regex = { git = "https://github.com/ardax-corp/coil-regex.git", version = "^0.1" }

[module]
roots = ["./src", "./.spool/deps/regex/src"]

[ffi]
search_paths = ["./.spool/deps/regex/native"]
```

Run `spool install`, then:

```coil
use regex::{compile, find_all, Regex};
```

**Docs:** [coil-regex](https://github.com/ardax-corp/coil-regex/blob/main/docs/README.md)

## Sibling checkout

In this repo, `coil-regex/` lives beside coil-lang. Point your `coil.toml` at it:

```toml
[module]
roots = ["./src", "../coil-regex/src"]

[ffi]
search_paths = ["../coil-regex/native"]
```

Build the native library: `make -C ../coil-regex/native`.

See [consume.md](https://github.com/ardax-corp/coil-regex/blob/main/docs/consume.md) for flags, `RegexError`, and `fn drop()` lifecycle.

## Migrating from virtual `regex`

The interpreter keeps nine **reserved** `regex_*` HostInvoke slots that panic if called (stale `.hyc`). New code must add coil-regex to `[module].roots` — `use regex::{compile}` without roots is a module-not-found error.

---

## Related

- [Getting Started](../manual/getting-started.md)
- [Modules](modules.md)
