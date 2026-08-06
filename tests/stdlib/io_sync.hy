use io::{open, close, write_from, IoError};
use io::sync::{write_all, read_to_end, read_exact};
use io::fs::{remove_file};
use string::{to_bytes, from_bytes};

test("write_from offset skips prefix") {
    let path = "/tmp/coil_stdlib_write_from.txt";
    let s = match open(path, "w") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open w",
    };
    let buf = to_bytes("XXXhello");
    match write_from(s, buf, 3) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "write_from",
    };
    match close(s) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close w",
    };
    let r = match open(path, "r") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open r",
    };
    let got = match read_to_end(r) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "read_to_end",
    };
    match close(r) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close r",
    };
    let text = match from_bytes(got) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "from_bytes",
    };
    assert(text == "hello")?;
    match remove_file(path) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "remove",
    };
}

test("write_from rejects bad offsets") {
    let path = "/tmp/coil_stdlib_write_from_bad.txt";
    let s = match open(path, "w") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open w",
    };
    let buf = to_bytes("abcd");
    match write_from(s, buf, 0 - 1) {
        Result::Ok(_) => panic "neg offset",
        Result::Err(IoError::InvalidInput) => 0,
        Result::Err(_) => panic "neg tag",
    };
    match write_from(s, buf, 5) {
        Result::Ok(_) => panic "past end",
        Result::Err(IoError::InvalidInput) => 0,
        Result::Err(_) => panic "past tag",
    };
    match write_from(s, buf, 4) {
        Result::Ok(n) => assert(n == 0)?,
        Result::Err(_) => panic "at len",
    };
    match close(s) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close",
    };
    match remove_file(path) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "remove",
    };
}

test("write_all roundtrip") {
    let path = "/tmp/coil_stdlib_write_all.txt";
    let payload = to_bytes("stdlib-sync-ok");
    let s = match open(path, "w") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open w",
    };
    match write_all(s, payload) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "write_all",
    };
    match close(s) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close w",
    };
    let r = match open(path, "r") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open r",
    };
    let got = match read_to_end(r) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "read_to_end",
    };
    match close(r) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close r",
    };
    let text = match from_bytes(got) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "from_bytes",
    };
    assert(text == "stdlib-sync-ok")?;
    match remove_file(path) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "remove",
    };
}

test("read_exact fills buffer") {
    let path = "/tmp/coil_stdlib_read_exact.txt";
    let payload = to_bytes("abcdef");
    let s = match open(path, "w") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open w",
    };
    match write_all(s, payload) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "write_all",
    };
    match close(s) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close w",
    };
    let r = match open(path, "r") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open r",
    };
    let buf = to_bytes("XXXX");
    match read_exact(r, buf) {
        Result::Ok(Option::Some(n)) => assert(n == 4)?,
        Result::Ok(Option::None) => panic "eof",
        Result::Err(_) => panic "read_exact",
    };
    match close(r) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close r",
    };
    let text = match from_bytes(buf) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "from_bytes",
    };
    assert(text == "abcd")?;
    match remove_file(path) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "remove",
    };
}

test("write_all empty buffer") {
    let path = "/tmp/coil_stdlib_write_all_empty.txt";
    let s = match open(path, "w") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open w",
    };
    let empty: Vec<byte> = Vec::new();
    match write_all(s, empty) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "write_all",
    };
    match close(s) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close w",
    };
    let r = match open(path, "r") {
        Result::Ok(v) => v,
        Result::Err(_) => panic "open r",
    };
    let got = match read_to_end(r) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "read_to_end",
    };
    match close(r) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "close r",
    };
    assert(len(got) == 0)?;
    match remove_file(path) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "remove",
    };
}
