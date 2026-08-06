// String helpers built on UTF-8 byte buffers (userland).
// Byte-oriented offsets — slicing mid-codepoint yields UTF-8 errors.
// Named `text` because virtual `string` already owns `format` / `to_bytes` / `from_bytes`.
use string::{to_bytes, from_bytes};
use bytes::{
    slice as bytes_slice,
    find as bytes_find,
    find_from as bytes_find_from,
    replace as bytes_replace,
    pad_left as bytes_pad_left,
    pad_right as bytes_pad_right,
    contains as bytes_contains,
    starts_with as bytes_starts_with,
    ends_with as bytes_ends_with,
    eq as bytes_eq,
};
use ascii::{is_space};

fn utf8_ok(Vec<byte> b) -> Result<string, string> {
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
    let cont = 1;
    while cont == 1 {
        if lo >= hi {
            cont = 0;
        } else {
            if is_space(b[lo]) {
                lo = lo + 1;
            } else {
                cont = 0;
            }
        }
    }
    cont = 1;
    while cont == 1 {
        if hi <= lo {
            cont = 0;
        } else {
            if is_space(b[hi - 1]) {
                hi = hi - 1;
            } else {
                cont = 0;
            }
        }
    }
    return utf8_ok(bytes_slice(b, lo, hi))?;
}

/// True when `hay` contains `needle` as a byte-exact substring.
fn contains(string hay, string needle) -> bool {
    return bytes_contains(to_bytes(hay), to_bytes(needle));
}

/// True when `s` begins with `prefix` (byte identity).
fn starts_with(string s, string prefix) -> bool {
    return bytes_starts_with(to_bytes(s), to_bytes(prefix));
}

/// True when `s` ends with `suffix` (byte identity).
fn ends_with(string s, string suffix) -> bool {
    return bytes_ends_with(to_bytes(s), to_bytes(suffix));
}

/// First byte offset of `needle` in `hay`, or `-1`. Empty needle → `0`.
fn find(string hay, string needle) -> int {
    return bytes_find(to_bytes(hay), to_bytes(needle));
}

/// Split `s` on every occurrence of `sep` (byte-exact). Empty sep → `[s]`.
fn split(string s, string sep) -> Result<Vec<string>, string> {
    let out: Vec<string> = Vec::new();
    let hay = to_bytes(s);
    let needle = to_bytes(sep);
    if len(needle) == 0 {
        out.push(s);
        return out;
    }
    let start = 0;
    let done = false;
    let hn = len(hay);
    while !done {
        let at = bytes_find_from(hay, needle, start);
        if at < 0 {
            let part = utf8_ok(bytes_slice(hay, start, hn))?;
            out.push(part);
            done = true;
        }
        if at >= 0 {
            let part = utf8_ok(bytes_slice(hay, start, at))?;
            out.push(part);
            start = at + len(needle);
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
fn join(Vec<string> parts, string sep) -> string {
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
fn lines(string s) -> Result<Vec<string>, string> {
    let b = to_bytes(s);
    let out: Vec<string> = Vec::new();
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
            let piece = match from_bytes(bytes_slice(b, start, end)) {
                Result::Ok(x) => x,
                Result::Err(_) => raise "utf8",
            };
            out.push(piece);
            start = i + 1;
        }
        i = i + 1;
    }
    let end = n;
    if end > start {
        if b[end - 1] == "\r" {
            end = end - 1;
        }
    }
    if start <= n {
        let piece = match from_bytes(bytes_slice(b, start, end)) {
            Result::Ok(x) => x,
            Result::Err(_) => raise "utf8",
        };
        out.push(piece);
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
    let out: Vec<byte> = Vec::new();
    let i = 0;
    let a_up: byte = "A";
    let z_up: byte = "Z";
    while i < len(b) {
        let c = b[i];
        if c >= a_up {
            if c <= z_up {
                let n = (c as int) + 32;
                let lo = n as byte;
                out.push(lo);
            }
            if c > z_up {
                out.push(c);
            }
        }
        if c < a_up {
            out.push(c);
        }
        i = i + 1;
    }
    return utf8_ok(out)?;
}

/// ASCII upper-case a..=z only; other bytes unchanged.
fn to_upper(string s) -> Result<string, string> {
    let b = to_bytes(s);
    let out: Vec<byte> = Vec::new();
    let i = 0;
    let a_lo: byte = "a";
    let z_lo: byte = "z";
    while i < len(b) {
        let c = b[i];
        if c >= a_lo {
            if c <= z_lo {
                let n = (c as int) - 32;
                let up = n as byte;
                out.push(up);
            }
            if c > z_lo {
                out.push(c);
            }
        }
        if c < a_lo {
            out.push(c);
        }
        i = i + 1;
    }
    return utf8_ok(out)?;
}
