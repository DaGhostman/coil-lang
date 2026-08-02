// Expected: compile failure — missing field on dict access.
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let x = { foo: 42 };
    write_all(stdout(), to_bytes(format("%i", x.bar)));
}
