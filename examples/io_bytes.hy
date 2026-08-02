// `byte` / `[byte]` basics used by the IO layer.
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let b: byte = 255;
    let arr: [byte] = [1, 2, 3];
    write_all(stdout(), to_bytes(format("%i", b)));
    write_all(stdout(), to_bytes(format("%i", len(arr))));
    write_all(stdout(), to_bytes(format("%i", arr[1])));
}
