use gc::{collect, get, root, unroot, upgrade, weak};
use io::{stdout};
use io::sync::{write_all};
use string::{to_bytes};

fn ephemeral_weak() {
    let r = root([1, 2, 3]);
    let w = weak(get(r));
    // Drop the strong root without retaining the payload locally.
    unroot(r);
    return w;
}

fn main() {
    let w = ephemeral_weak();
    collect();
    let label = match upgrade(w) {
        Option::Some(_) => "some",
        Option::None => "none",
    };
    write_all(stdout(), to_bytes(label));
}
