use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let y = 10;
    let f = fn (int x) use (y) => x + y;
    write_all(stdout(), to_bytes(format("%i", f(32))));
}
