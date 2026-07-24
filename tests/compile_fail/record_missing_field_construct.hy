// Expected: compile failure — missing field in record constructor.
enum Point {
    Point { x: int, y: int },
}

fn main() {
    let p = Point::Point { x: 1 };
}
