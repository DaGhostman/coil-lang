// Expected output: 12342
//
// Irrefutable let destructuring: tuple and record patterns.

fn main() {
    let (a, b) = (1, 2);
    print "%i", a;
    print "%i", b;

    let { x, y } = { x: 3, y: 4 };
    print "%i", x;
    print "%i", y;
    // Nested tuple inside a record field.
    let { pair } = { pair: (2, 0) };
    let (p, _) = pair;
    print "%i", p;
}
