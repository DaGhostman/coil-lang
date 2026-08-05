// Blocking IO adapters over L0 + `await_*` (userland; not host natives).
//
// Match-arm bodies that start with `let` / `return` / `if` can be parsed as
// record literals — keep arms to assignments / trailing expressions, then
// branch after the match. Avoid nested `match` on `Option` inside a
// `Result::Ok` arm (bindings may not apply); unwrap or match Option outside.
use io::{
    read,
    write,
    await_readable as wait_readable,
    await_writable as wait_writable,
    stdout,
    stderr,
    from_bytes as io_from_bytes,
};
use io::net::tcp::accept;
use io::net::udp::recv_from;
use string::{to_bytes as str_to_bytes};

fn write_all(Stream s, [byte] buf) -> Result<int, IoError> {
    let offset = 0;
    let total = len(buf);
    while offset < total {
        let rest: [byte] = [];
        let i = offset;
        while i < total {
            rest[] = buf[i];
            i = i + 1;
        }
        let nwritten = 0;
        let got_ok = 0;
        let got_wb = 0;
        match write(s, rest) {
            Result::Ok(n) => {
                got_ok = 1;
                nwritten = n;
            },
            Result::Err(IoError::WouldBlock) => {
                got_wb = 1;
            },
            Result::Err(e) => {
                got_ok = 0;
                raise e;
            },
        };
        if got_wb == 1 {
            wait_writable(s)?;
        }
        if got_ok == 1 {
            if nwritten == 0 {
                wait_writable(s)?;
            }
            if nwritten != 0 {
                offset = offset + nwritten;
            }
        }
    }
    return 0;
}

fn read_exact(Stream s, [byte] buf) -> Result<Option<int>, IoError> {
    let need = len(buf);
    let filled = 0;
    while filled < need {
        let remaining = need - filled;
        let scratch: [byte] = [];
        let i = 0;
        while i < remaining {
            scratch[] = 0;
            i = i + 1;
        }
        // `?` propagates WouldBlock; callers that need wait should retry.
        // Prefer this over nested Result/Option matches (bindings can misfire).
        let rr = read(s, scratch)?;
        let is_none = 0;
        let nread = 0;
        match rr {
            Option::None => {
                is_none = 1;
            },
            Option::Some(n) => {
                nread = n;
            },
        };
        if is_none == 1 {
            if filled == 0 {
                return Option::None;
            }
            return Option::Some(filled);
        }
        if nread == 0 {
            wait_readable(s)?;
        }
        if nread != 0 {
            let j = 0;
            while j < nread {
                buf[filled + j] = scratch[j];
                j = j + 1;
            }
            filled = filled + nread;
        }
    }
    return Option::Some(filled);
}

fn read_to_end(Stream s) -> Result<[byte], IoError> {
    let acc: [byte] = [];
    let chunk_size = 256;
    let done = 0;
    while done == 0 {
        let scratch: [byte] = [];
        let i = 0;
        while i < chunk_size {
            scratch[] = 0;
            i = i + 1;
        }
        let rr = read(s, scratch)?;
        let is_none = 0;
        let nread = 0;
        match rr {
            Option::None => {
                is_none = 1;
            },
            Option::Some(n) => {
                nread = n;
            },
        };
        if is_none == 1 {
            done = 1;
        }
        if is_none == 0 {
            if nread == 0 {
                wait_readable(s)?;
            }
            if nread != 0 {
                let j = 0;
                while j < nread {
                    acc[] = scratch[j];
                    j = j + 1;
                }
            }
        }
    }
    return acc;
}

fn accept_wait(Stream listener) -> Result<Stream, IoError> {
    let done = 0;
    let out = listener;
    while done == 0 {
        let got = 0;
        let got_wb = 0;
        match accept(listener) {
            Result::Ok(s) => {
                out = s;
                got = 1;
            },
            Result::Err(IoError::WouldBlock) => {
                got_wb = 1;
            },
            Result::Err(e) => {
                got = 0;
                raise e;
            },
        };
        if got == 1 {
            done = 1;
        }
        if got_wb == 1 {
            wait_readable(listener)?;
        }
    }
    return out;
}

fn recv_from_wait(Stream s, [byte] buf) -> Result<(int, string, int), IoError> {
    let done = 0;
    let out_n = 0;
    let out_host = "";
    let out_port = 0;
    while done == 0 {
        let got = 0;
        let got_wb = 0;
        match recv_from(s, buf) {
            Result::Ok(t) => {
                out_n = t[0];
                out_host = t[1];
                out_port = t[2];
                got = 1;
            },
            Result::Err(IoError::WouldBlock) => {
                got_wb = 1;
            },
            Result::Err(e) => {
                got = 0;
                raise e;
            },
        };
        if got == 1 {
            done = 1;
        }
        if got_wb == 1 {
            wait_readable(s)?;
        }
    }
    return (out_n, out_host, out_port);
}

// --- text / line helpers (stdlib ergonomics) ---

fn newline_bytes() -> [byte] {
    let nl: [byte] = [];
    nl[] = 10;
    return nl;
}

/// Write UTF-8 bytes of `s` to stdout (no trailing newline).
fn print(string s) -> Result<int, IoError> {
    return write_all(stdout(), str_to_bytes(s))?;
}

/// Write `s` plus a trailing LF to stdout.
fn println(string s) -> Result<int, IoError> {
    write_all(stdout(), str_to_bytes(s))?;
    return write_all(stdout(), newline_bytes())?;
}

/// Write `s` plus a trailing LF to stderr.
fn eprintln(string s) -> Result<int, IoError> {
    write_all(stderr(), str_to_bytes(s))?;
    return write_all(stderr(), newline_bytes())?;
}

/// Read until LF (10) or EOF. Returns `None` on EOF with no bytes read.
/// The trailing LF is not included; a lone CR before LF is stripped.
fn read_line(Stream s) -> Result<Option<string>, IoError> {
    let acc: [byte] = [];
    let scratch: [byte] = [];
    scratch[] = 0;
    let done = 0;
    let saw = 0;
    let lf: byte = 10;
    let cr: byte = 13;
    while done == 0 {
        let rr = read(s, scratch)?;
        let is_none = 0;
        let nread = 0;
        match rr {
            Option::None => {
                is_none = 1;
            },
            Option::Some(n) => {
                nread = n;
            },
        };
        if is_none == 1 {
            done = 1;
        }
        if is_none == 0 {
            if nread == 0 {
                wait_readable(s)?;
            }
            if nread != 0 {
                saw = 1;
                let c = scratch[0];
                if c == lf {
                    done = 1;
                }
                if c != lf {
                    acc[] = c;
                }
            }
        }
    }
    if saw == 0 {
        return Option::None;
    }
    // Strip trailing CR if present (CRLF).
    if len(acc) > 0 {
        if acc[len(acc) - 1] == cr {
            let trimmed: [byte] = [];
            let i = 0;
            while i + 1 < len(acc) {
                trimmed[] = acc[i];
                i = i + 1;
            }
            return Option::Some(io_from_bytes(trimmed)?);
        }
    }
    return Option::Some(io_from_bytes(acc)?);
}
