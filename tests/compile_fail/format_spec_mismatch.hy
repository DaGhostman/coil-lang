// Expected: compile failure — %s requires string.
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    write_all(stdout(), to_bytes(format("%s", 42)));
}
