async fn counter() {
    yield 0;
    yield 1;
    yield 2;
}

async fn wrap() {
    yield from counter();
}

fn main() {
    let h = wrap();
    let v0 = resume h;
    print "%i", v0;
    let v1 = resume h;
    print "%i", v1;
    let v2 = resume h;
    print "%i", v2;
}
