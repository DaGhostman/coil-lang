async fn counter() {
    yield 0;
    yield 1;
    yield 2;
    return 99;
}

async fn early() {
    yield 10;
    yield 20;
    yield 30;
}

fn main() {
    for x in counter() {
        print "%i", x;
    }
    for y in early() {
        if y == 20 {
            break;
        }
        print "%i", y;
    }
}
