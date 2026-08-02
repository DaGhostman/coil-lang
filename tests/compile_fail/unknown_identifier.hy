// Expected: compile failure — unknown value.
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    write_all(stdout(), to_bytes(format("%i", not_defined)));
}
