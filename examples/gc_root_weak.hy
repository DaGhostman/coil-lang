use gc::*;
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

fn main() {
    let r = root("pinned");
    let w = weak(get(r));
    let label = match upgrade(w) {
        Option::Some(s) => s,
        Option::None => "gone",
    };
    write_all(stdout(), to_bytes(label));
    let taken = unroot(r);
    write_all(stdout(), to_bytes(format("\n%s", taken)));
}
