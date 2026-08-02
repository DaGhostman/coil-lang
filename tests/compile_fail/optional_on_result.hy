// Expected: compile failure — ?. on Result.
use io::{stdout, write_all};
use string::{format, to_bytes};
fn main() {
    let r = Result::Ok({ v: 1 });
    write_all(stdout(), to_bytes(format("%v", r?.v)));
}
