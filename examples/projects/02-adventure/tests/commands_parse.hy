use commands::{cmd_dir, cmd_kind, parse_line};

test("parse look") {
    let look: [byte] = [108, 111, 111, 107];
    let c0 = parse_line(look);
    assert(cmd_kind(c0) == 0)?;
}

test("parse go north") {
    let go_n: [byte] = [103, 111, 32, 110, 111, 114, 116, 104];
    let c1 = parse_line(go_n);
    assert(cmd_kind(c1) == 1)?;
    assert(cmd_dir(c1) == 0)?;
}

test("parse go south east west") {
    let go_s: [byte] = [103, 111, 32, 115, 111, 117, 116, 104];
    let go_e: [byte] = [103, 111, 32, 101, 97, 115, 116];
    let go_w: [byte] = [103, 111, 32, 119, 101, 115, 116];
    let c_s = parse_line(go_s);
    assert(cmd_kind(c_s) == 1)?;
    assert(cmd_dir(c_s) == 1)?;
    let c_e = parse_line(go_e);
    assert(cmd_kind(c_e) == 1)?;
    assert(cmd_dir(c_e) == 2)?;
    let c_w = parse_line(go_w);
    assert(cmd_kind(c_w) == 1)?;
    assert(cmd_dir(c_w) == 3)?;
}

test("parse take inventory quit exit") {
    let take_key: [byte] = [116, 97, 107, 101, 32, 107, 101, 121];
    let inv: [byte] = [105, 110, 118, 101, 110, 116, 111, 114, 121];
    let quit: [byte] = [113, 117, 105, 116];
    let exit: [byte] = [101, 120, 105, 116];
    let c_take = parse_line(take_key);
    assert(cmd_kind(c_take) == 2)?;
    let c_inv = parse_line(inv);
    assert(cmd_kind(c_inv) == 3)?;
    let c_quit = parse_line(quit);
    assert(cmd_kind(c_quit) == 7)?;
    let c_exit = parse_line(exit);
    assert(cmd_kind(c_exit) == 7)?;
}

test("parse unknown") {
    let bad: [byte] = [120];
    let c2 = parse_line(bad);
    assert(cmd_kind(c2) == 8)?;
}
