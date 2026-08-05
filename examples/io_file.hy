// File round-trip via virtual `io` module.
// Writes two bytes, reads them back with read_to_end, prints length.
use io::*;
use io::sync::{read_to_end, write_all};

use string::*;

fn write_file(string path, [byte] data) {
    let s = open(path, "w")?;
    write_all(s, data)?;
    close(s)?;
    return 0;
}

fn read_len(string path) {
    let s = open(path, "r")?;
    let buf = read_to_end(s)?;
    close(s)?;
    return len(buf);
}

fn run(string path, [byte] data) {
    write_file(path, data)?;
    let n = read_len(path)?;
    return format("%i", n);
}

fn main() {
    let path = "/tmp/coil_io_file_test.bin";
    let data: [byte] = [72, 105];
    write_all(stdout(), to_bytes(format("%s", match run(path, data) {
        Result::Ok(s) => s,
        Result::Err(_) => "err",
    })));
}
