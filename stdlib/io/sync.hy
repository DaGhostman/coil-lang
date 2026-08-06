// Blocking IO adapters over L0 + `await_*` (userland; not host natives).
use io::{
    read,
    write_from,
    await_readable as wait_readable,
    await_writable as wait_writable,
    stdout,
    stderr,
    from_bytes as io_from_bytes,
};
use io::net::tcp::accept;
use io::net::udp::recv_from;
use string::{to_bytes as str_to_bytes};

/// Write every byte of `buf` to `s`, parking on `WouldBlock` until done.
fn write_all(Stream s, Vec<byte> buf) -> Result<int, IoError> {
    let offset = 0;
    let total = len(buf);
    while offset < total {
        match write_from(s, buf, offset) {
            Result::Ok(n) => {
                if n == 0 {
                    wait_writable(s)?;
                }
                if n != 0 {
                    offset = offset + n;
                }
            },
            Result::Err(IoError::WouldBlock) => {
                wait_writable(s)?;
            },
            Result::Err(e) => {
                raise e;
            },
        };
    }
    return 0;
}

/// Fill `buf` from `s`; returns bytes read or `None` on EOF before fill.
fn read_exact(Stream s, Vec<byte> buf) -> Result<Option<int>, IoError> {
    let need = len(buf);
    let filled = 0;
    while filled < need {
        // Scratch length must equal remaining: L0 `read` fills the whole buffer.
        let remaining = need - filled;
        let scratch: Vec<byte> = Vec::new();
        let i = 0;
        while i < remaining {
            scratch.push(0);
            i = i + 1;
        }
        match read(s, scratch)? {
            Option::None => {
                if filled == 0 {
                    return Option::None;
                }
                return Option::Some(filled);
            },
            Option::Some(n) => {
                if n == 0 {
                    wait_readable(s)?;
                }
                if n != 0 {
                    let j = 0;
                    while j < n {
                        buf[filled + j] = scratch[j];
                        j = j + 1;
                    }
                    filled = filled + n;
                }
            },
        };
    }
    return Option::Some(filled);
}

/// Read from `s` until EOF into a new `Vec<byte>`.
fn read_to_end(Stream s) -> Result<Vec<byte>, IoError> {
    let acc: Vec<byte> = Vec::new();
    let chunk_size = 4096;
    let scratch: Vec<byte> = Vec::new();
    let i = 0;
    while i < chunk_size {
        scratch.push(0);
        i = i + 1;
    }
    let done = false;
    while !done {
        match read(s, scratch)? {
            Option::None => {
                done = true;
            },
            Option::Some(n) => {
                if n == 0 {
                    wait_readable(s)?;
                }
                if n != 0 {
                    let j = 0;
                    while j < n {
                        acc.push(scratch[j]);
                        j = j + 1;
                    }
                }
            },
        };
    }
    return acc;
}

/// Blocking `accept` on `listener`, parking on `WouldBlock`.
fn accept_wait(Stream listener) -> Result<Stream, IoError> {
    let done = false;
    let out = listener;
    while !done {
        match accept(listener) {
            Result::Ok(s) => {
                out = s;
                done = true;
            },
            Result::Err(IoError::WouldBlock) => {
                wait_readable(listener)?;
            },
            Result::Err(e) => {
                raise e;
            },
        };
    }
    return out;
}

/// Blocking UDP `recv_from`, parking on `WouldBlock`; returns `(n, host, port)`.
fn recv_from_wait(Stream s, Vec<byte> buf) -> Result<(int, string, int), IoError> {
    let done = false;
    let out_n = 0;
    let out_host = "";
    let out_port = 0;
    while !done {
        match recv_from(s, buf) {
            Result::Ok(t) => {
                out_n = t[0];
                out_host = t[1];
                out_port = t[2];
                done = true;
            },
            Result::Err(IoError::WouldBlock) => {
                wait_readable(s)?;
            },
            Result::Err(e) => {
                raise e;
            },
        };
    }
    return (out_n, out_host, out_port);
}

fn newline_bytes() -> Vec<byte> {
    let nl: Vec<byte> = Vec::new();
    nl.push("\n");
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
    let acc: Vec<byte> = Vec::new();
    // One-byte reads: a larger scratch would consume past LF without pushback.
    let scratch: Vec<byte> = Vec::new();
    scratch.push(0);
    let done = false;
    let saw = false;
    let lf: byte = "\n";
    let cr: byte = "\r";
    while !done {
        match read(s, scratch)? {
            Option::None => {
                done = true;
            },
            Option::Some(n) => {
                if n == 0 {
                    wait_readable(s)?;
                }
                if n != 0 {
                    saw = true;
                    let c = scratch[0];
                    if c == lf {
                        done = true;
                    }
                    if c != lf {
                        acc.push(c);
                    }
                }
            },
        };
    }
    if !saw {
        return Option::None;
    }
    if len(acc) > 0 {
        if acc[len(acc) - 1] == cr {
            let trimmed: Vec<byte> = Vec::new();
            let i = 0;
            while i + 1 < len(acc) {
                trimmed.push(acc[i]);
                i = i + 1;
            }
            return Option::Some(io_from_bytes(trimmed)?);
        }
    }
    return Option::Some(io_from_bytes(acc)?);
}
