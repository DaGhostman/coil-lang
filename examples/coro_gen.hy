async fn counter() {
    yield 0;
    yield 1;
    yield 2;
}

fn main() {
    let h = counter();
    print "%i", resume h;
    print "%i", resume h;
    print "%i", resume h;
}
