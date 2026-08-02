// Expected: compile failure — field access on non-record type.
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let x = 1;
    write_all(stdout(), to_bytes(format("%i", x.foo)));
}
