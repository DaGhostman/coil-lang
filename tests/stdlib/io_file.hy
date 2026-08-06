use io::file::{read_text, write_text, read_bytes, write_bytes};
use io::fs::{exists, remove_file};

test("read write text roundtrip") {
    let path = "/tmp/coil_stdlib_file_test.txt";
    match write_text(path, "hello") {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "write_text",
    };
    let ex = match exists(path) {
        Result::Ok(v) => v,
        Result::Err(_) => panic "exists",
    };
    assert(ex)?;
    let t = match read_text(path) {
        Result::Ok(s) => s,
        Result::Err(_) => panic "read_text",
    };
    assert(t == "hello")?;
    match remove_file(path) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "remove",
    };
}
