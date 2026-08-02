// Expected output: 42hi1.5true(3,4)99
//
// `%v` displays values through the `Show` trait. Builtin instances
// cover int/float/string/bool/unit; user types can `impl Show for T`.

use io::{stdout, write_all};
use string::{format, to_bytes};
enum Point {
    Point { x: int, y: int },
}

impl Show for Point {
    fn show(Point p) -> string {
        return format("(%i,%i)", p.x, p.y);
    }
}

fn show_it<T: Show>(T x) {
    write_all(stdout(), to_bytes(format("%v", x)));
}

fn main() {
    show_it(42);
    show_it("hi");
    show_it(1.5);
    show_it(true);
    write_all(stdout(), to_bytes(format("%v", Point::Point { x: 3, y: 4 })));
    let s = format("%v", 99);
    write_all(stdout(), to_bytes(format("%s", s)));
}
