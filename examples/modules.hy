// examples/modules.hy — Phase 29A: namespace system live.
//
// `use foo::sadge;` brings `sadge` (a function in
// `src/foo/sadge.hy`) into scope. The module's name
// (`foo`) is resolved via the manifest's search roots
// (default: `src/`). The function's fully qualified
// name is `foo::sadge::sadge`; the alias `sadge`
// resolves to that FQN at the call site.
//
// Expected output:
//   "1a4\n"   (sadge prints 420 in hex, then newline)
//   "45"     (69 in hex, from the inline print)

use foo::sadge;

fn main() {
    sadge();
    print "%x\n", 69;
}
