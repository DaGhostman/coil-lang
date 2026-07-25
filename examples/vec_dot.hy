// Named helpers — `dot` and `cross` on homogeneous vectors.
// Expected output: 32,001

fn main() {
    let d = dot((1, 2, 3), (4, 5, 6));
    print "%i,", d;
    let c = cross((1, 0, 0), (0, 1, 0));
    print "%i%i%i", c[0], c[1], c[2];
}
