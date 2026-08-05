// Blocking IO adapters over L0 + `await_*` (userland; not host natives).
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
        match write(s, rest) {
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

fn read_to_end(Stream s) -> Result<[byte], IoError> {
    let acc: [byte] = [];
    let chunk_size = 256;
    let done = false;
    while !done {
        let scratch: [byte] = [];
        let i = 0;
        while i < chunk_size {
            scratch[] = 0;
            i = i + 1;
        }
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
                        acc[] = scratch[j];
                        j = j + 1;
                    }
                }
            },
        };
    }
    return acc;
}

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

fn recv_from_wait(Stream s, [byte] buf) -> Result<(int, string, int), IoError> {
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

fn newline_bytes() -> [byte] {
    let nl: [byte] = [];
    nl[] = "\n";
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
                        acc[] = c;
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
