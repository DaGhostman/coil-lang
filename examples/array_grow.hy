use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let a = [1, 2];
    a[] = 3;
    a[] = 4;
    write_all(stdout(), to_bytes(format("%i", len(a))));
    write_all(stdout(), to_bytes(format("%i", a[0])));
    write_all(stdout(), to_bytes(format("%i", a[3])));
}
