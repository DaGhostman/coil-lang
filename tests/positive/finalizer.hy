use gc::{collect, root, weak, upgrade, Weak};

class Handle {
    fd: int,
}

static let drops: int = 0;
static let during: int = 0;
static let held: Option<Weak<Handle>> = Option::None;

impl Handle {
    fn drop() {
        let live = match held {
            Option::Some(w) => match upgrade(w) {
                Option::Some(_) => 1,
                Option::None => 2,
            },
            Option::None => 0,
        };
        during = live;
        collect();
        drops = drops + 1;
    }
}

fn ephemeral() {
    let h = new Handle(1);
}

fn with_weak() {
    let h = new Handle(5);
    held = Option::Some(weak(h));
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

test("weak stays live during drop and is None after sweep") {
    drops = 0;
    during = 0;
    with_weak();
    collect();
    assert(during == 1)?;
    let after = match held {
        Option::Some(w) => match upgrade(w) {
            Option::Some(_) => 1,
            Option::None => 0,
        },
        Option::None => -1,
    };
    assert(after == 0)?;
}
