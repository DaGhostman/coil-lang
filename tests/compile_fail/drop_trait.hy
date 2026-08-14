// Expected: compile failure — fn drop is not a trait method (E0126).
trait Closer {
    fn drop() {}
}

fn main() {}
