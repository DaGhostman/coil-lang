async fn coro() {
    print "Suspended\n";
    yield 1;
    print "Resumed\n";
}

fn main() {
    let h = coro();
    let x = resume h;
    print "%i", x;
    resume h;
}
