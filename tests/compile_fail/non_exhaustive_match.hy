// Expected: compile failure — non-exhaustive match.
use io::{stdout, write_all};
use string::{format, to_bytes};
enum Color {
    Red,
    Green,
    Blue,
}

fn main() {
    let c = Color::Red;
    write_all(stdout(), to_bytes(format("%i", match c {
        Color::Red => 0,
    })));
}
