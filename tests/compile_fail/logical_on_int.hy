// Expected: compile failure — && requires bool operands.
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};
fn main() {
    write_all(stdout(), to_bytes(format("%z", 1 && 2)));
}
