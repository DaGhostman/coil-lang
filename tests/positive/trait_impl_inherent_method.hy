// Expected output: 1
//
// Trait-instance methods may call inherent methods declared later (COI-115).

use io::{stdout, write};
use string::{format, to_bytes};

class ItemBox {
    v: int,
}

class ItemBoxIter {
    i: int,
}

impl IntoIterator<ItemBox> {
    type Item = int;
    type IntoIter = ItemBoxIter;
    fn into_iter(ItemBox m) -> ItemBoxIter {
        return m.iter();
    }
}

impl ItemBox {
    fn iter() -> ItemBoxIter {
        return new ItemBoxIter(self.v);
    }
}

impl Iterator<ItemBoxIter> {
    type Item = int;
    fn next(ItemBoxIter it) -> Option<int> {
        if it.i == 0 {
            it.i = 1;
            return Option::Some(1);
        }
        return Option::None;
    }
}

fn main() {
    let b = new ItemBox(0);
    for x in b {
        write(stdout(), to_bytes(format("%i", x)));
    }
}
