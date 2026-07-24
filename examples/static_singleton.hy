static let hits = 0;

class Counter {
    static count: int = 0,
    value: int,
}

impl Counter {
    fn bump() {
        Counter::count = Counter::count + 1;
        self.value = self.value + 1;
    }
}

fn main() {
    hits = hits + 1;
    Counter::count = Counter::count + 1;
    let c = new Counter(0);
    c.bump();
    print "%i", hits;
    print "%i", Counter::count;
    print "%i", c.value;
}
