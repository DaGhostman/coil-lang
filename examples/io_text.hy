// Decode `[byte]` → string with `from_bytes`, encode with `to_bytes`.
use io::*;
use string::*;

fn main() {
    // "hello" as ASCII bytes
    let hello: [byte] = [104, 101, 108, 108, 111];
    write_all(stdout(), to_bytes(format("%s", match from_bytes(hello) {
        Result::Ok(s) => s,
        Result::Err(_) => "err",
    })));

    let back = to_bytes("hi");
    write_all(stdout(), to_bytes(format("%i", len(back))));
}
