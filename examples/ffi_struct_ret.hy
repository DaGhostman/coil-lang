// examples/ffi_struct_ret.hy — FFI struct return via make_point.
//
// Output: 34  (Point { x: 3, y: 4 } fields)

use ffi::*;
use ffi::types::*;

extern struct Point {
    x: int32,
    y: int32,
};

fn main() -> Result<(), Error> {
    let lib = dload("sum")?;
    let make_id = declare(lib, "make_point", (Int32, Int32), Point)?;
    let p = invoke(lib, make_id, (3, 4))?;
    print "%i", p.x;
    print "%i", p.y;
}
