// examples/coro_done.hy — done(h) builtin.
//
// Output: falsefalsetrue

async fn steps() {
    yield 1;
    yield 2;
}

fn main() {
    let h = steps();
    print "%z", done(h);
    resume h;
    print "%z", done(h);
    resume h;
    resume h; // completes
    print "%z", done(h);
}
