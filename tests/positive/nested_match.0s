// Inner-pattern dispatch + nested record patterns.
enum Opt {
    Nada,
    Yea(int),
}

enum Res {
    Good(Opt),
    Bad(string),
}

enum Inner {
    I { v: int },
}

enum Wrap {
    W { inner: Inner, name: string },
}

fn unwrap_res(Res r) -> int {
    return match r {
        Res::Good(Opt::Yea(v)) => v,
        Res::Good(Opt::Nada) => 0,
        Res::Bad(_) => -1,
    };
}

fn get_v(Wrap w) -> int {
    return match w {
        Wrap::W { inner: Inner::I { v }, name } => v,
    };
}

test("inner pattern some") {
    assert(unwrap_res(Res::Good(Opt::Yea(42))) == 42)?;
}

test("inner pattern none") {
    assert(unwrap_res(Res::Good(Opt::Nada)) == 0)?;
}

test("inner pattern err") {
    assert(unwrap_res(Res::Bad("x")) == -1)?;
}

test("nested record pattern") {
    let w = Wrap::W { inner: Inner::I { v: 99 }, name: "x" };
    assert(get_v(w) == 99)?;
}
