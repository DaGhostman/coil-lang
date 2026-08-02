use time::*;
use io::{stdout, write_all};
use string::{format, to_bytes};

fn epoch_ok() -> int {
    return match epoch() {
        Result::Ok(t) => 1,
        Result::Err(e) => 0,
    };
}

fn main() {
    write_all(stdout(), to_bytes(format("%i", epoch_ok())));
}
