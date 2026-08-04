// Exercise io::drive and await_* registration (no hung wait).
use io::{stdout, write_all, drive};
use string::{format, to_bytes};

fn main() {
    let n = drive();
    write_all(stdout(), to_bytes(format("%i", n)));
}
