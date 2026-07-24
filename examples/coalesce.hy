// ?? coalesce on Option and Result (Result Err is swallowed).
fn main() {
    let a = Option::None ?? "bar";
    print "%s,", a;
    let b = Option::Some("hi") ?? "bar";
    print "%s,", b;
    let c = Result::Err("boom") ?? 7;
    print "%i,", c;
    let d = Result::Ok(9) ?? 7;
    print "%i", d;
}
