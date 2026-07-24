// attr_ffi.hy — `#[ffi]` attribute sugar for compile-time libc bindings.
//
// Expected output: `5` (strlen of "hello").

#[ffi(lib = "c")]
fn strlen(string s) -> int;

fn main() {
    let n = strlen("hello");
    print "%i", n;
}
