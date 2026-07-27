use thread::*;

fn work() -> int {
    return 40 + 2;
}

fn main() {
    let t = spawn(work)?;
    print "%i", join(t)?;
}
