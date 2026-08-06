// Path string helpers (userland). Forward slashes preferred; also accepts `\`.
use io::{IoError};
use string::{to_bytes, from_bytes};
use bytes::{slice as bytes_slice, concat as bytes_concat};

fn is_sep(byte c) -> bool {
    let slash: byte = "/";
    let bslash: byte = "\\";
    if c == slash {
        return true;
    }
    if c == bslash {
        return true;
    }
    return false;
}

/// Join `a` and `b` with a single `/` when needed.
fn join(string a, string b) -> Result<string, IoError> {
    if len(b) == 0 {
        return a;
    }
    if len(a) == 0 {
        return b;
    }
    let ab = to_bytes(a);
    let bb = to_bytes(b);
    let a_ends = is_sep(ab[len(ab) - 1]);
    let b_starts = is_sep(bb[0]);
    if a_ends {
        if b_starts {
            return from_bytes(bytes_concat(ab, bytes_slice(bb, 1, len(bb))))?;
        }
        return from_bytes(bytes_concat(ab, bb))?;
    }
    if b_starts {
        return from_bytes(bytes_concat(ab, bb))?;
    }
    let slash: Vec<byte> = Vec::new();
    slash.push(47);
    return from_bytes(bytes_concat(bytes_concat(ab, slash), bb))?;
}

/// Parent directory, or `"."` when there is no separator.
fn dirname(string path) -> Result<string, IoError> {
    let b = to_bytes(path);
    let n = len(b);
    if n == 0 {
        return ".";
    }
    let end = n;
    while end > 1 {
        if is_sep(b[end - 1]) {
            end = end - 1;
        }
        if end > 1 {
            if is_sep(b[end - 1]) == false {
                break;
            }
        }
        if end <= 1 {
            break;
        }
    }
    let i = end;
    while i > 0 {
        i = i - 1;
        if is_sep(b[i]) {
            if i == 0 {
                return from_bytes(bytes_slice(b, 0, 1))?;
            }
            return from_bytes(bytes_slice(b, 0, i))?;
        }
    }
    return ".";
}

/// Final path component (after last separator).
fn basename(string path) -> Result<string, IoError> {
    let b = to_bytes(path);
    let n = len(b);
    if n == 0 {
        return "";
    }
    let end = n;
    while end > 0 {
        if is_sep(b[end - 1]) {
            end = end - 1;
        }
        if end > 0 {
            if is_sep(b[end - 1]) == false {
                break;
            }
        }
        if end <= 0 {
            break;
        }
    }
    if end == 0 {
        return "";
    }
    let i = end;
    while i > 0 {
        i = i - 1;
        if is_sep(b[i]) {
            return from_bytes(bytes_slice(b, i + 1, end))?;
        }
    }
    return from_bytes(bytes_slice(b, 0, end))?;
}

/// Extension after the last `.` in the basename, or empty string.
fn extension(string path) -> Result<string, IoError> {
    let base = basename(path)?;
    let b = to_bytes(base);
    let n = len(b);
    let i = n;
    while i > 0 {
        i = i - 1;
        if b[i] == "." {
            if i + 1 >= n {
                return "";
            }
            return from_bytes(bytes_slice(b, i + 1, n))?;
        }
    }
    return "";
}

/// True when `path` begins with `/` or a Windows drive `X:`.
fn is_absolute(string path) -> bool {
    let b = to_bytes(path);
    if len(b) == 0 {
        return false;
    }
    if is_sep(b[0]) {
        return true;
    }
    if len(b) >= 2 {
        if b[0] >= "A" {
            if b[0] <= "Z" {
                if b[1] == ":" {
                    return true;
                }
            }
        }
        if b[0] >= "a" {
            if b[0] <= "z" {
                if b[1] == ":" {
                    return true;
                }
            }
        }
    }
    return false;
}
