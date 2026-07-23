attr log<T>(fn(...args) -> T target, string message, ...args) -> T {
    print "%s", message;
    return target(...args);
}

#[log(message = "Point ctor")]
class Point {
    x: int,
    y: int,
}

fn main() {
    let p = new Point(5, 12);
    print "%i", p.x;
    print "%i", p.y;
}
