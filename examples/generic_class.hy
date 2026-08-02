// examples/generic_class.hy — generic class + impl end-to-end.
//
// Output: 42

use io::{stdout, write_all};
use string::{format, to_bytes};
class Cell<T> {
    value: T
}

impl Cell<T> {
    fn get() -> T {
        return self.value;
    }
}

fn main() {
    let c = new Cell(42);
    write_all(stdout(), to_bytes(format("%i", c.get())));
}
