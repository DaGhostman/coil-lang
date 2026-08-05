// String helpers built on UTF-8 byte buffers (userland).
// Byte-oriented offsets — slicing mid-codepoint yields UTF-8 errors.
// Named `text` because virtual `string` already owns `format` / `to_bytes` / `from_bytes`.
use string::{to_bytes, from_bytes};
use bytes::{
    slice as bytes_slice,
    find as bytes_find,
    replace as bytes_replace,
    pad_left as bytes_pad_left,
    pad_right as bytes_pad_right,
    contains as bytes_contains,
    starts_with as bytes_starts_with,
    ends_with as bytes_ends_with,
    eq as bytes_eq,
};
use ascii::{is_space};

fn utf8_ok([byte] b) -> Result<string, string> {
    return match from_bytes(b) {
        Result::Ok(s) => s,
        Result::Err(_) => raise "utf8",
    };
}

/// Byte length of UTF-8 `s` (same as `len(to_bytes(s))`).
fn byte_len(string s) -> int {
    return len(to_bytes(s));
}

/// Slice by byte offsets; returns `Err` if the slice is not valid UTF-8.
fn slice(string s, int start, int end) -> Result<string, string> {
    return utf8_ok(bytes_slice(to_bytes(s), start, end))?;
}

/// Trim ASCII whitespace (space/tab/CR/LF) from both ends.
fn trim(string s) -> Result<string, string> {
    let b = to_bytes(s);
    let lo = 0;
    let hi = len(b);
    while lo < hi {
        if is_space(b[lo]) {
            lo = lo + 1;
        }
        if lo < hi {
            if is_space(b[lo]) == false {
                break;
            }
        }
        if lo >= hi {
            break;
        }
    }
    while hi > lo {
        if is_space(b[hi - 1]) {
            hi = hi - 1;
        }
        if hi > lo {
            if is_space(b[hi - 1]) == false {
                break;
            }
        }
        if hi <= lo {
            break;
        }
    }
    return utf8_ok(bytes_slice(b, lo, hi))?;
}

fn contains(string hay, string needle) -> bool {
    return bytes_contains(to_bytes(hay), to_bytes(needle));
}

fn starts_with(string s, string prefix) -> bool {
    return bytes_starts_with(to_bytes(s), to_bytes(prefix));
}

fn ends_with(string s, string suffix) -> bool {
    return bytes_ends_with(to_bytes(s), to_bytes(suffix));
}

fn find(string hay, string needle) -> int {
    return bytes_find(to_bytes(hay), to_bytes(needle));
}

/// Split `s` on every occurrence of `sep` (byte-exact). Empty sep → `[s]`.
fn split(string s, string sep) -> Result<[string], string> {
    let out: [string] = [];
    let hay = to_bytes(s);
    let needle = to_bytes(sep);
    if len(needle) == 0 {
        out[] = s;
        return out;
    }
    let start = 0;
    let done = false;
    while !done {
        let rest = bytes_slice(hay, start, len(hay));
        let rel = bytes_find(rest, needle);
        if rel < 0 {
            let part = utf8_ok(rest)?;
            out[] = part;
            done = true;
        }
        if rel >= 0 {
            let part = utf8_ok(bytes_slice(hay, start, start + rel))?;
            out[] = part;
            start = start + rel + len(needle);
        }
    }
    return out;
}

/// Split at the first occurrence of `sep`, excluding the separator.
fn split_once(string s, string sep) -> Result<(string, string), string> {
    let hay = to_bytes(s);
    let needle = to_bytes(sep);
    let at = bytes_find(hay, needle);
    if at < 0 {
        raise "separator not found";
    }
    let left = utf8_ok(bytes_slice(hay, 0, at))?;
    let right = utf8_ok(bytes_slice(hay, at + len(needle), len(hay)))?;
    return (left, right);
}

/// Replace every non-overlapping occurrence of `old` with `new`.
fn replace(string s, string old, string new) -> Result<string, string> {
    let out = bytes_replace(to_bytes(s), to_bytes(old), to_bytes(new));
    return utf8_ok(out)?;
}

/// Join strings with `sep` between adjacent parts.
fn join([string] parts, string sep) -> string {
    let out = "";
    let i = 0;
    while i < len(parts) {
        if i > 0 {
            out = out + sep;
        }
        out = out + parts[i];
        i = i + 1;
    }
    return out;
}

/// Repeat `s` `n` times. Non-positive counts produce an empty string.
fn repeat(string s, int n) -> string {
    let out = "";
    let i = 0;
    while i < n {
        out = out + s;
        i = i + 1;
    }
    return out;
}

/// Pad on the left to a byte width using a one-byte `fill` string.
fn pad_left(string s, int width, string fill) -> Result<string, string> {
    let fill_bytes = to_bytes(fill);
    if len(fill_bytes) != 1 {
        raise "fill must be one byte";
    }
    return utf8_ok(bytes_pad_left(to_bytes(s), width, fill_bytes[0]))?;
}

/// Pad on the right to a byte width using a one-byte `fill` string.
fn pad_right(string s, int width, string fill) -> Result<string, string> {
    let fill_bytes = to_bytes(fill);
    if len(fill_bytes) != 1 {
        raise "fill must be one byte";
    }
    return utf8_ok(bytes_pad_right(to_bytes(s), width, fill_bytes[0]))?;
}

/// Split on LF and strip one optional CR from each resulting line.
fn lines(string s) -> Result<[string], string> {
    let b = to_bytes(s);
    let out: [string] = [];
    let start = 0;
    let i = 0;
    let n = len(b);
    while i < n {
        if b[i] == "\n" {
            let end = i;
            if end > start {
                if b[end - 1] == "\r" {
                    end = end - 1;
                }
            }
            out[] = utf8_ok(bytes_slice(b, start, end))?;
            start = i + 1;
        }
        i = i + 1;
    }
    // Trailing segment (including empty after a final LF).
    let end = n;
    if end > start {
        if b[end - 1] == "\r" {
            end = end - 1;
        }
    }
    if start <= n {
        out[] = utf8_ok(bytes_slice(b, start, end))?;
    }
    return out;
}

/// Concatenate two strings.
fn concat(string a, string b) -> string {
    return a + b;
}

/// True when strings are equal (byte identity).
fn eq(string a, string b) -> bool {
    return bytes_eq(to_bytes(a), to_bytes(b));
}

/// ASCII lower-case A..=Z only; other bytes unchanged.
fn to_lower(string s) -> Result<string, string> {
    let b = to_bytes(s);
    let out: [byte] = [];
    let i = 0;
    let a_up: byte = "A";
    let z_up: byte = "Z";
    while i < len(b) {
        let c = b[i];
        if c >= a_up {
            if c <= z_up {
                let n = (c as int) + 32;
                let lo = n as byte;
                out[] = lo;
            }
            if c > z_up {
                out[] = c;
            }
        }
        if c < a_up {
            out[] = c;
        }
        i = i + 1;
    }
    return utf8_ok(out)?;
}

/// ASCII upper-case a..=z only; other bytes unchanged.
fn to_upper(string s) -> Result<string, string> {
    let b = to_bytes(s);
    let out: [byte] = [];
    let i = 0;
    let a_lo: byte = "a";
    let z_lo: byte = "z";
    while i < len(b) {
        let c = b[i];
        if c >= a_lo {
            if c <= z_lo {
                let n = (c as int) - 32;
                let up = n as byte;
                out[] = up;
            }
            if c > z_lo {
                out[] = c;
            }
        }
        if c < a_lo {
            out[] = c;
        }
        i = i + 1;
    }
    return utf8_ok(out)?;
}
