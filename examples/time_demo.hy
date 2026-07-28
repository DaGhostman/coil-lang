use time::*;

fn epoch_ok() -> int {
    return match epoch() {
        Result::Ok(t) => 1,
        Result::Err(e) => 0,
    };
}

fn main() {
    print "%i", epoch_ok();
}
