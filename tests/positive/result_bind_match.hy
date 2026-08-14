// COI-106: method Result returns must box before local bind + match.

class Svc {}

enum Node {
    Obj { v: int },
}

impl Svc {
    fn decode() -> Result<Node, string> {
        return Node::Obj { v: 42 };
    }
}

test("method result bind then match") {
    let s = new Svc();
    let r = s.decode();
    let ok = match r {
        Result::Ok(_) => true,
        Result::Err(_) => false,
    };
    assert(ok)?;
}
