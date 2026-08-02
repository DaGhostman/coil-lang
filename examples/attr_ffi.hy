// attr_ffi.hy — `#[ffi]` attribute sugar for compile-time libc bindings.
//
// Expected output: `5` (strlen of "hello").

use io::{stdout, write_all};
use string::{format, to_bytes};
#[ffi(lib = "c")]
fn strlen(string s) -> int;

fn main() {
    let n = strlen("hello");
    write_all(stdout(), to_bytes(format("%i", n)));
}
