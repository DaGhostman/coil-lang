// Whole-file helpers over `io::open` + `io::sync` (userland).
use io::{open, close, IoError};
use io::sync::{read_to_end, write_all};
use string::{to_bytes, from_bytes};

/// Read an entire file as bytes (`open` + `read_to_end` + `close`).
fn read_bytes(string path) -> Result<Vec<byte>, IoError> {
    let s = open(path, "r")?;
    let buf = match read_to_end(s) {
        Result::Ok(b) => b,
        Result::Err(e) => {
            match close(s) {
                Result::Ok(_) => 0,
                Result::Err(_) => 0,
            };
            raise e;
        },
    };
    close(s)?;
    return buf;
}

/// Write `buf` to `path`, creating/truncating (`"w"`).
fn write_bytes(string path, Vec<byte> buf) -> Result<int, IoError> {
    let s = open(path, "w")?;
    match write_all(s, buf) {
        Result::Ok(_) => 0,
        Result::Err(e) => {
            match close(s) {
                Result::Ok(_) => 0,
                Result::Err(_) => 0,
            };
            raise e;
        },
    };
    close(s)?;
    return 0;
}

/// Read an entire file as a UTF-8 string.
fn read_text(string path) -> Result<string, IoError> {
    let b = read_bytes(path)?;
    return match from_bytes(b) {
        Result::Ok(s) => s,
        Result::Err(e) => raise e,
    };
}

/// Write a UTF-8 string to `path`.
fn write_text(string path, string text) -> Result<int, IoError> {
    let b = to_bytes(text);
    return write_bytes(path, b)?;
}
