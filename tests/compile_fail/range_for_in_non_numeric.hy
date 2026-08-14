// Expected: compile failure — for-in needs int/byte/float, not generic Ord.
fn dump<T: Ord>(T a, T b) {
    for x in a..=b { }
}

fn main() {}
