// Expected: compile failure — cannot index non-aggregate.
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let x = 5;
    write_all(stdout(), to_bytes(format("%i", x[0])));
}
