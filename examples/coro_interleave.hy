// Two handles from the SAME parameterized async fn, interleaved,
// with `resume` used inline directly as a `print` argument.
async fn counter(int base) {
    yield base;
    yield base + 1;
    yield base + 2;
}

fn main() {
    let a = counter(10);
    let b = counter(100);

    // Interleave resumes: a, b, b, a, a, b
    print "%i,", resume a;
    print "%i,", resume b;
    print "%i,", resume b;
    print "%i,", resume a;
    print "%i,", resume a;
    print "%i", resume b;
}
