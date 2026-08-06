use io::{open, close, write_from};
use io::sync::{write_all, read_to_end};
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
