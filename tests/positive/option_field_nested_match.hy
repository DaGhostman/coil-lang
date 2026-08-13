// Nested `match` on the same Option field copies the field; outer
// pattern bindings stay in scope in the inner arm.
class BoxInt {
    opt: Option<int>,
}

class Node {
    val: int,
    left: Option<Node>,
    right: Option<Node>,
}

class Holder {
    text: Option<string>,
}

fn nested_same_field(BoxInt b) -> int {
    return match b.opt {
        Option::Some(v) => match b.opt {
            Option::Some(v2) => v + v2,
            Option::None => -1,
        },
        Option::None => 0,
    };
}

fn sequential_same_field(BoxInt b) -> int {
    let first = match b.opt {
        Option::Some(v) => v,
        Option::None => -1,
    };
    let second = match b.opt {
        Option::Some(v) => v,
        Option::None => -2,
    };
    return first * 100 + second;
}

fn nested_niche_child(Node n) -> int {
    return match n.left {
        Option::Some(child) => match n.left {
            Option::Some(child2) => child.val + child2.val,
            Option::None => -1,
        },
        Option::None => 0,
    };
}

fn nested_shadows_inner_name(BoxInt b) -> int {
    return match b.opt {
        Option::Some(v) => match Option::Some(100) {
            Option::Some(v) => v,
            Option::None => -1,
        },
        Option::None => 0,
    };
}

fn nested_none_arm_uses_outer(BoxInt b) -> int {
    return match b.opt {
        Option::Some(v) => match Option::None {
            Option::Some(_) => -1,
            Option::None => v,
        },
        Option::None => 0,
    };
}

test("nested match on boxed Option field") {
    let b = new BoxInt(Option::Some(21));
    assert(nested_same_field(b) == 42)?;
}

test("matching a field does not consume it") {
    let b = new BoxInt(Option::Some(21));
    assert(sequential_same_field(b) == 2121)?;
}

test("nested match on niche Option class field") {
    let leaf = new Node(3, Option::None, Option::None);
    let root = new Node(1, Option::Some(leaf), Option::None);
    assert(nested_niche_child(root) == 6)?;
}

test("inner binding shadows outer match name") {
    let b = new BoxInt(Option::Some(21));
    assert(nested_shadows_inner_name(b) == 100)?;
}

test("inner None arm still sees outer binding") {
    let b = new BoxInt(Option::Some(21));
    assert(nested_none_arm_uses_outer(b) == 21)?;
}

test("nested match on niche Option string field") {
    let h = new Holder(Option::Some("ok"));
    let n = match h.text {
        Option::Some(s) => match h.text {
            Option::Some(_) => s,
            Option::None => "gone",
        },
        Option::None => "none",
    };
    assert(n == "ok")?;
}
