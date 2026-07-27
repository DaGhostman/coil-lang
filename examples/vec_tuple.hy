// Numeric tower — homogeneous tuple zip / broadcast / negate.
// Expected output: 22,23,24,-1-2

fn main() {
    let a = (1, 1) + (1, 1);
    print "%i%i,", a[0], a[1];
    let b = (1, 2) + 1;
    print "%i%i,", b[0], b[1];
    let c = 2 * (1, 2);
    print "%i%i,", c[0], c[1];
    let d = -(1, 2);
    print "%i%i", d[0], d[1];
}
