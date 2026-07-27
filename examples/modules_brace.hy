// examples/modules_brace.hy — brace-group imports from a module file.
//
// `examples/src/math.hy` exports `add` and `mul`. Brace syntax imports
// both in one statement; the resolver falls back to the module file
// (`math.hy`) because there is no `math/add.hy` / `math/mul.hy`.
//
// Expected output: `1242`

use math::{add, mul};

fn main() {
    print "%i", add(5, 7);
    print "%i", mul(6, 7);
}
