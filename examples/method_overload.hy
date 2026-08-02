use io::{stdout, write_all};
use string::{format, to_bytes};
class Counter {
    value: int,
}

impl Counter {
    fn bump(int by) -> int {
        self.value = self.value + by;
        return self.value;
    }

    fn bump() -> int {
        return self.bump(1);
    }
}

fn main() {
    let c = new Counter(10);
    write_all(stdout(), to_bytes(format("%i", c.bump())));
    write_all(stdout(), to_bytes(format("%i", c.bump(5))));
}
