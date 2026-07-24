// examples/derive_show_eq.hy — `#[derive(...)]` for Show / Eq / Ord.
//
// Output: Color::Red,true,false,true,Point::Point { x: 5, y: 12 },true,false,Cell { value: 42 },true,false

#[derive(Show, Eq, Ord)]
enum Color {
    Red,
    Blue,
}

#[derive(Show, Eq)]
enum Point {
    Origin,
    Point { x: int, y: int },
}

#[derive(Show, Eq)]
class Cell {
    value: int,
}

fn main() {
    print "%v,", Color::Red;
    print "%z,", Color::Red == Color::Red;
    print "%z,", Color::Red == Color::Blue;
    print "%z,", Color::Red < Color::Blue;

    let p = Point::Point { x: 5, y: 12 };
    print "%v,", p;
    print "%z,", p == Point::Point { x: 5, y: 12 };
    print "%z,", p == Point::Origin;

    let c = new Cell(42);
    print "%v,", c;
    print "%z,", c == new Cell(42);
    print "%z", c == new Cell(7);
}
