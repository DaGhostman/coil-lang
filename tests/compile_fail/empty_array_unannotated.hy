// Expected: compile failure — empty `[]` needs `Vec<T>` or `[T; 0]`.
fn main() {
    let xs = [];
}
