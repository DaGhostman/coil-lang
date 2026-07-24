// Expected: compile failure — non-exhaustive match.
enum Color {
    Red,
    Green,
    Blue,
}

fn main() {
    let c = Color::Red;
    print "%i", match c {
        Color::Red => 0,
    };
}
