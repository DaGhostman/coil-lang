# `regex` module

`use regex::{compile, is_match, find, replace};` — PCRE2 patterns via HostInvoke (system **libpcre2** / `pcre2-sys`). Opaque `Regex` handle from `compile(pattern, flags)`.

| Surface | Types |
|---------|--------|
| `compile` | `(string, string) -> Result<Regex, RegexError>` |
| `is_match` | `(Regex, string) -> Result<bool, RegexError>` |
| `find` | `(Regex, string) -> Result<(int, int), RegexError>` — first match byte span; no match → `NoMatch` |
| `find_all` | `(Regex, string) -> Result<[(int, int)], RegexError>` — all non-overlapping spans (empty if none) |
| `captures` | `(Regex, string) -> Result<[string], RegexError>` — `[0]` full match; empty string for non-participating groups |
| `captures_all` | `(Regex, string) -> Result<[[string]], RegexError>` |
| `split` | `(Regex, string) -> Result<[string], RegexError>` |
| `replace` / `replace_all` | `(Regex, string, string) -> Result<string, RegexError>` — `$n` / `${name}` / `$$` |

**Flags** (second `compile` arg; case-sensitive; unknown letter → `Compile`): `i` caseless, `m` multiline, `s` dotall, `x` extended, `u` Unicode properties (`ucp`). UTF-8 matching is always on for coil strings. Other PCRE letters (`A`/`D`/`U`/`J`/…) are not exposed — use in-pattern verbs where PCRE2 allows.

`RegexError` variants: `Compile`, `Runtime`, `NoMatch`, `Utf8`.

---

## Related

- [Getting Started](../manual/getting-started.md)
