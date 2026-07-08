// sum.c — a minimal C library exposed to userland via the
// FFI mechanism. Defines a single `sum` function that adds
// two integers. Compiled to a shared library and loaded by
// the VM at startup via `dlopen` + `dlsym`.
//
// Build:
//   cc -shared -fPIC -o libsum.so sum.c
// then put `libsum.so` somewhere the dynamic linker can find
// (e.g. the current directory) so `dlopen("sum")` resolves
// to it.

/// `int sum(int a, int b)` — return the sum of two integers.
///
/// Symbol name: `sum` (the function's identifier as it
/// appears in userland). The zero-script FFI declaration
/// matches by name:
///
/// ```0s
/// extern "sum" {
///     fn sum(int a, int b) -> int;
/// }
/// ```
///
/// The VM's `dlopen("sum")` call looks for `libsum.so` on
/// the system library search path (or the current
/// directory). The `dlsym("sum")` call resolves the symbol
/// in that library. The VM then wraps the resolved function
/// pointer in a `LibraryFn` and dispatches `NATIVE` opcodes
/// that name `sum` to it.
int sum(int a, int b) {
    return a + b;
}
