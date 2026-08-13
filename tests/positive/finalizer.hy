use gc::{collect, root, weak, upgrade, Weak};

class Handle {
    fd: int,
}

class Resurrect {
    fd: int,
}

static let drops: int = 0;
static let during: int = 0;
static let held: Option<Weak<Handle>> = Option::None;
static let resurrect_drops: int = 0;
static let kept: Option<Resurrect> = Option::None;

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

impl Resurrect {
    fn drop() {
        resurrect_drops = resurrect_drops + 1;
        kept = Option::Some(self);
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

fn stash_self() {
    let h = new Resurrect(42);
}

fn resurrected_fd() -> int {
    return match kept {
        Option::Some(h) => h.fd,
        Option::None => -1,
    };
}

fn clear_kept() {
    kept = Option::None;
}

test("storing self from drop resurrects once") {
    resurrect_drops = 0;
    kept = Option::None;
    stash_self();
    collect();
    assert(resurrect_drops == 1)?;
    assert(resurrected_fd() == 42)?;
    clear_kept();
    collect();
    assert(resurrect_drops == 1)?;
}

fn explicit_stash() {
    let h = new Resurrect(7);
    h.drop();
}

test("explicit drop storing self stays once") {
    resurrect_drops = 0;
    kept = Option::None;
    explicit_stash();
    collect();
    assert(resurrect_drops == 1)?;
    clear_kept();
    collect();
    assert(resurrect_drops == 1)?;
}
