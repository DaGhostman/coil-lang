use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let sum = 0;
    for (let i = 0; i < 10; i = i + 1) {
        if i == 3 { continue; }
        if i == 7 { break; }
        sum = sum + i;
    }
    write_all(stdout(), to_bytes(format("%i", sum)));
}
