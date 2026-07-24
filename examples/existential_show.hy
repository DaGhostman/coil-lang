// Bare-class existential: `Show` as a value type.
// Expected output: 42

fn print_any(Show x) {
    print "%s", show(x);
}

fn main() {
    print_any(42);
}
