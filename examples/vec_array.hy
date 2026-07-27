// Numeric tower — static array zip / scalar broadcast.
// Literals infer `[int; N]`, so zip is allowed. Dynamic `[T] ⊕ [T]` is a
// hard type error (see diagnostics tests).
// Expected output: 46,45,18

fn main() {
    let a = [1, 2] + [3, 4];
    print "%i%i,", a[0], a[1];
    let b = [1, 2] + 3;
    print "%i%i,", b[0], b[1];
    let c = [1, 2] ** 3;
    print "%i%i", c[0], c[1];
}
