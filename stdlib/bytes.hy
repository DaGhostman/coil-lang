// Byte-buffer helpers (userland). Indices are byte offsets; no UTF-8 awareness.
use string::{to_bytes, from_bytes};

/// Copy `src[start..end)` into a new buffer (clamped to `src` bounds).
fn slice([byte] src, int start, int end) -> [byte] {
    let out: [byte] = [];
    let i = start;
    if i < 0 {
        i = 0;
    }
    while i < end {
        if i < len(src) {
            out[] = src[i];
        }
        i = i + 1;
    }
    return out;
}

/// Append `b` after `a` into a new buffer.
fn concat([byte] a, [byte] b) -> [byte] {
    let out: [byte] = [];
    let i = 0;
    while i < len(a) {
        out[] = a[i];
        i = i + 1;
    }
    let j = 0;
    while j < len(b) {
        out[] = b[j];
        j = j + 1;
    }
    return out;
}

/// True when `a` and `b` have equal length and equal bytes.
fn eq([byte] a, [byte] b) -> bool {
    if len(a) != len(b) {
        return false;
    }
    let i = 0;
    while i < len(a) {
        if a[i] != b[i] {
            return false;
        }
        i = i + 1;
    }
    return true;
}

/// First index of `needle` in `hay`, or `-1` if missing. Empty needle → `0`.
fn find([byte] hay, [byte] needle) -> int {
    let hn = len(hay);
    let nn = len(needle);
    if nn == 0 {
        return 0;
    }
    if nn > hn {
        return -1;
    }
    let i = 0;
    while i + nn <= hn {
        let ok = true;
        let j = 0;
        while j < nn {
            if hay[i + j] != needle[j] {
                ok = false;
            }
            j = j + 1;
        }
        if ok {
            return i;
        }
        i = i + 1;
    }
    return -1;
}

/// True when `hay` contains `needle` as a contiguous sub-buffer.
fn contains([byte] hay, [byte] needle) -> bool {
    return find(hay, needle) >= 0;
}

/// True when `buf` begins with `prefix`.
fn starts_with([byte] buf, [byte] prefix) -> bool {
    let n = len(prefix);
    if n > len(buf) {
        return false;
    }
    return eq(slice(buf, 0, n), prefix);
}

/// True when `buf` ends with `suffix`.
fn ends_with([byte] buf, [byte] suffix) -> bool {
    let n = len(suffix);
    let m = len(buf);
    if n > m {
        return false;
    }
    return eq(slice(buf, m - n, m), suffix);
}

/// Copy every byte of `src` into a new buffer.
fn copy([byte] src) -> [byte] {
    return slice(src, 0, len(src));
}

/// Decode UTF-8 bytes (maps `string::from_bytes` errors to a bare string Err).
fn to_string([byte] b) -> Result<string, string> {
    return match from_bytes(b) {
        Result::Ok(s) => s,
        Result::Err(_) => raise "utf8",
    };
}

/// Encode a string as UTF-8 bytes (alias of `string::to_bytes`).
fn from_string(string s) -> [byte] {
    return to_bytes(s);
}
