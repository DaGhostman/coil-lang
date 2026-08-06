// Byte-buffer helpers (userland). Indices are byte offsets; no UTF-8 awareness.
use string::{to_bytes, from_bytes};

/// True when `a[ao..ao+n)` equals `b[bo..bo+n)` (caller guarantees bounds).
fn region_eq(Vec<byte> a, int ao, Vec<byte> b, int bo, int n) -> bool {
    let i = 0;
    let ok = true;
    while ok && i < n {
        if a[ao + i] != b[bo + i] {
            ok = false;
        }
        if ok {
            i = i + 1;
        }
    }
    return ok;
}

/// Copy `src[start..end)` into a new buffer (clamped to `src` bounds).
fn slice(Vec<byte> src, int start, int end) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let i = start;
    if i < 0 {
        i = 0;
    }
    while i < end {
        if i < len(src) {
            out.push(src[i]);
        }
        i = i + 1;
    }
    return out;
}

/// Append `b` after `a` into a new buffer.
fn concat(Vec<byte> a, Vec<byte> b) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let i = 0;
    while i < len(a) {
        out.push(a[i]);
        i = i + 1;
    }
    let j = 0;
    while j < len(b) {
        out.push(b[j]);
        j = j + 1;
    }
    return out;
}

/// True when `a` and `b` have equal length and equal bytes.
fn eq(Vec<byte> a, Vec<byte> b) -> bool {
    if len(a) != len(b) {
        return false;
    }
    return region_eq(a, 0, b, 0, len(a));
}

/// First index of `needle` in `hay` at or after `start`, or `-1`.
/// Empty needle → `start` clamped into `[0, len(hay)]`.
fn find_from(Vec<byte> hay, Vec<byte> needle, int start) -> int {
    let hn = len(hay);
    let nn = len(needle);
    let i = start;
    if i < 0 {
        i = 0;
    }
    if i > hn {
        i = hn;
    }
    if nn == 0 {
        return i;
    }
    if nn > hn {
        return -1;
    }
    while i + nn <= hn {
        if region_eq(hay, i, needle, 0, nn) {
            return i;
        }
        i = i + 1;
    }
    return -1;
}

/// First index of `needle` in `hay`, or `-1` if missing. Empty needle → `0`.
fn find(Vec<byte> hay, Vec<byte> needle) -> int {
    return find_from(hay, needle, 0);
}

/// Last index of `needle` in `hay`, or `-1` if missing. Empty needle → `len(hay)`.
fn rfind(Vec<byte> hay, Vec<byte> needle) -> int {
    let hn = len(hay);
    let nn = len(needle);
    if nn == 0 {
        return hn;
    }
    if nn > hn {
        return -1;
    }
    let i = hn - nn;
    while i >= 0 {
        if region_eq(hay, i, needle, 0, nn) {
            return i;
        }
        i = i - 1;
    }
    return -1;
}

/// True when `hay` contains `needle` as a contiguous sub-buffer.
fn contains(Vec<byte> hay, Vec<byte> needle) -> bool {
    return find(hay, needle) >= 0;
}

/// True when `buf` begins with `prefix`.
fn starts_with(Vec<byte> buf, Vec<byte> prefix) -> bool {
    let n = len(prefix);
    if n > len(buf) {
        return false;
    }
    return region_eq(buf, 0, prefix, 0, n);
}

/// True when `buf` ends with `suffix`.
fn ends_with(Vec<byte> buf, Vec<byte> suffix) -> bool {
    let n = len(suffix);
    let m = len(buf);
    if n > m {
        return false;
    }
    return region_eq(buf, m - n, suffix, 0, n);
}

/// Copy every byte of `src` into a new buffer.
fn copy(Vec<byte> src) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let i = 0;
    while i < len(src) {
        out.push(src[i]);
        i = i + 1;
    }
    return out;
}

/// Replace all non-overlapping occurrences of `old` with `new`.
/// An empty `old` leaves `hay` unchanged.
fn replace(Vec<byte> hay, Vec<byte> old, Vec<byte> new) -> Vec<byte> {
    if len(old) == 0 {
        return copy(hay);
    }
    let out: Vec<byte> = Vec::new();
    let i = 0;
    let on = len(old);
    while i < len(hay) {
        let matches = i + on <= len(hay);
        if matches {
            matches = region_eq(hay, i, old, 0, on);
        }
        if matches {
            let k = 0;
            while k < len(new) {
                out.push(new[k]);
                k = k + 1;
            }
            i = i + on;
        } else {
            out.push(hay[i]);
            i = i + 1;
        }
    }
    return out;
}

/// Repeat `src` `n` times. Non-positive counts produce an empty buffer.
fn repeat(Vec<byte> src, int n) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let count = 0;
    while count < n {
        let i = 0;
        while i < len(src) {
            out.push(src[i]);
            i = i + 1;
        }
        count = count + 1;
    }
    return out;
}

/// Pad on the left with `fill` until the buffer reaches `width`.
fn pad_left(Vec<byte> src, int width, byte fill) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let padding = width - len(src);
    let i = 0;
    while i < padding {
        out.push(fill);
        i = i + 1;
    }
    let j = 0;
    while j < len(src) {
        out.push(src[j]);
        j = j + 1;
    }
    return out;
}

/// Pad on the right with `fill` until the buffer reaches `width`.
fn pad_right(Vec<byte> src, int width, byte fill) -> Vec<byte> {
    let out: Vec<byte> = Vec::new();
    let j = 0;
    let n = len(src);
    while j < n {
        out.push(src[j]);
        j = j + 1;
    }
    let padding = width - n;
    let i = 0;
    while i < padding {
        out.push(fill);
        i = i + 1;
    }
    return out;
}

/// Decode UTF-8 bytes (maps `string::from_bytes` errors to a bare string Err).
fn to_string(Vec<byte> b) -> Result<string, string> {
    return match from_bytes(b) {
        Result::Ok(s) => s,
        Result::Err(_) => raise "utf8",
    };
}

/// Encode a string as UTF-8 bytes (alias of `string::to_bytes`).
fn from_string(string s) -> Vec<byte> {
    return to_bytes(s);
}
