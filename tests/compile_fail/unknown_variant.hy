// Expected: compile failure — unknown variant.
enum Color {
    Red,
}

fn main() {
    let c = Color::Blue;
}
