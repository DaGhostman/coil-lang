// String helpers built on UTF-8 byte buffers (userland).
// Byte-oriented offsets — slicing mid-codepoint yields UTF-8 errors.
// Named `text` because virtual `string` already owns `format` / `to_bytes` / `from_bytes`.
use string::{to_bytes, from_bytes};
use bytes::{
    slice as bytes_slice,
    find as bytes_find,
    contains as bytes_contains,
    starts_with as bytes_starts_with,
    ends_with as bytes_ends_with,
    eq as bytes_eq,
};

fn is_ascii_space(byte c) -> bool {
    let sp: byte = 32;
    let tab: byte = 9;
    let lf: byte = 10;
    let cr: byte = 13;
    if c == sp {
        return true;
    }
    if c == tab {
        return true;
    }
    if c == lf {
        return true;
    }
    if c == cr {
        return true;
    }
    return false;
}

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
        if is_ascii_space(b[lo]) {
            lo = lo + 1;
        }
        if lo < hi {
            if is_ascii_space(b[lo]) == false {
                break;
            }
        }
        if lo >= hi {
            break;
        }
    }
    while hi > lo {
        if is_ascii_space(b[hi - 1]) {
            hi = hi - 1;
        }
        if hi > lo {
            if is_ascii_space(b[hi - 1]) == false {
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
    let a_up: byte = 65;
    let z_up: byte = 90;
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
    let a_lo: byte = 97;
    let z_lo: byte = 122;
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
