use collections::list::{List};

test("list push pop") {
    let xs = List::new();
    assert(xs.is_empty())?;
    xs.push_front(1);
    xs.push_front(2);
    xs.push_front(3);
    assert(xs.size() == 3)?;
    assert(xs.peek_front_or(0) == 3)?;
    assert(xs.pop_front_or(0) == 3)?;
    assert(xs.pop_front_or(0) == 2)?;
    assert(xs.pop_front_or(0) == 1)?;
    assert(xs.pop_front_or(-1) == -1)?;
    assert(xs.is_empty())?;
    xs.push_front(9);
    xs.clear();
    assert(xs.is_empty())?;
}

test("list empty peek and clear reuse") {
    let xs = List::new();
    assert(xs.peek_front_or(-7) == -7)?;
    assert(xs.pop_front_or(-7) == -7)?;
    xs.push_front(1);
    xs.push_front(2);
    xs.clear();
    assert(xs.is_empty())?;
    assert(xs.peek_front_or(0) == 0)?;
    xs.push_front(5);
    assert(xs.size() == 1)?;
    assert(xs.peek_front_or(0) == 5)?;
}
