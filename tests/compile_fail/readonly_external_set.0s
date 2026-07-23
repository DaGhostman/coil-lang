// Expected: compile failure — external mutation of readonly value.
class Point {
    x: int,
    y: int,
}

fn main() {
    let p = new readonly Point(1, 2);
    p.x = 10;
}
