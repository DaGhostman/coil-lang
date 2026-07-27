// Nominal `Matrix` — `*` is matmul (Mul), `+` is element-wise.
// Expected output: 19,22,43,502

fn main() {
    let a = matrix([[1, 2], [3, 4]]);
    let b = matrix([[5, 6], [7, 8]]);
    let c = a * b;
    print "%i,%i,%i,%i", c[0][0], c[0][1], c[1][0], c[1][1];
    let d = a + a;
    print "%i", d[0][0];
}
