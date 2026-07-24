// Decode `[byte]` → string with `from_bytes`, encode with `to_bytes`.
use io::*;

fn main() {
    // "hello" as ASCII bytes
    let hello: [byte] = [104, 101, 108, 108, 111];
    print "%s", match from_bytes(hello) {
        Result::Ok(s) => s,
        Result::Err(_) => "err",
    };

    let back = to_bytes("hi");
    print "%i", len(back);
}
