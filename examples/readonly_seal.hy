class Point {
    x: int,
    y: int,
}

impl Point {
    fn shift() {
        self.x = self.x + 1;
    }
}

fn main() {
    let xs = readonly [1, 2, 3];
    print "%i", len(xs);
    let p = readonly new Point(1, 2);
    p.shift();
    print "%i", p.x;
    print "%i", p.y;
}
