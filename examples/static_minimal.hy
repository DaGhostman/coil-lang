use io::{stdout, write_all};
use string::{format, to_bytes};
static let hits = 0;

class Counter {
    static count: int = 0,
    value: int,
}

fn main() {
    hits = hits + 1;
    Counter::count = Counter::count + 1;
    write_all(stdout(), to_bytes(format("%i", hits)));
    write_all(stdout(), to_bytes(format("%i", Counter::count)));
}
