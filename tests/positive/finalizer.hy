use gc::{collect, root};

static let drops: int = 0;

class Handle {
    fd: int,
}

impl Handle {
    fn drop() {
        collect();
        drops = drops + 1;
    }
}

fn ephemeral() {
    let h = new Handle(1);
}

test("drop runs on collect") {
    drops = 0;
    ephemeral();
    collect();
    assert(drops == 1)?;
}

test("explicit drop counts once") {
    drops = 0;
    let h = new Handle(2);
    h.drop();
    h.drop();
    collect();
    assert(drops == 1)?;
}

test("live Root is not finalized") {
    drops = 0;
    let h = new Handle(3);
    let r = root(h);
    collect();
    assert(drops == 0)?;
}
