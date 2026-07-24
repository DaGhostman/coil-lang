// ?. optional field access + ?? fallback on a record (dict).
fn show(Option o) -> int {
    return match o {
        Option::Some(n) => n,
        Option::None => 0,
    };
}

fn main() {
    let some = Option::Some({ v: 42 });
    let none = Option::None;
    print "%i,", show(some?.v);
    print "%i", none?.v ?? 0;
}
