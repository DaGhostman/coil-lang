// User generic enum — same construct/match machinery as builtin Option.
enum Box<T> {
    Empty,
    Full(T),
}

fn unwrap(Box<int> b) -> int {
    return match b {
        Box::Empty => 0,
        Box::Full(v) => v,
    };
}

fn main() {
    print "%i", unwrap(Box::Full(7));
}
