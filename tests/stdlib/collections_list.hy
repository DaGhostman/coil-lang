use collections::list::{
    list_new,
    list_push_front,
    list_peek_front_or,
    list_pop_front_or,
    list_clear,
};

test("list push pop") {
    let xs = list_new();
    assert(xs.is_empty())?;
    list_push_front(xs, 1);
    list_push_front(xs, 2);
    list_push_front(xs, 3);
    assert(xs.size() == 3)?;
    assert(list_peek_front_or(xs, 0) == 3)?;
    assert(list_pop_front_or(xs, 0) == 3)?;
    assert(list_pop_front_or(xs, 0) == 2)?;
    assert(list_pop_front_or(xs, 0) == 1)?;
    assert(list_pop_front_or(xs, -1) == -1)?;
    assert(xs.is_empty())?;
    list_push_front(xs, 9);
    list_clear(xs);
    assert(xs.is_empty())?;
}
