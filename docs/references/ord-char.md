# `ord` and `char`

Auto-imported: `ord(string) -> Result<byte, string>` (exactly one character with codepoint ≤ 255) and `char(byte) -> Result<string, string>` (exactly one UTF-8 code unit). Out-of-range `char` inputs (not in `0..=255`) return `Err("byte out of range")`. Prefer keeping the argument typed as `byte`. String literals of one such character coerce to `byte` in annotations (e.g. `let c: byte = "A";`).

---

## Related

- [Types](types.md)
