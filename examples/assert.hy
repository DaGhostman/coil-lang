// prelude::test::assert — returns Result<(), string>.
fn check_ok(int n) {
    assert(n == 42)?;
    return "ok";
}

fn main() {
    print "%s,", match check_ok(42) {
        Result::Ok(v) => v,
        Result::Err(e) => e,
    };
    print "%s,", match assert(false) {
        Result::Ok(_) => "ok",
        Result::Err(e) => e,
    };
    print "%s", match assert(1 == 0, "custom") {
        Result::Ok(_) => "ok",
        Result::Err(e) => e,
    };
}
