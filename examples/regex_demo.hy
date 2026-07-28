// PCRE2 virtual module: compile flags, find_all, split, replace_all.
use regex::*;

fn must_re(Result r) -> Regex {
    return match r {
        Result::Ok(v) => v,
        Result::Err(_) => panic "regex compile failed",
    };
}

fn main() {
    let re = must_re(compile("(\\w+)=(\\d+)", "i"));
    print "%z,", match is_match(re, "X=42") {
        Result::Ok(b) => b,
        Result::Err(_) => false,
    };
    let spans = match find_all(re, "a=1 b=2") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "find_all",
    };
    print "%i,", len(spans);
    print "%s,", match replace_all(re, "a=1 b=2", "$1->$2") {
        Result::Ok(s) => s,
        Result::Err(_) => "",
    };
    let sep = must_re(compile(",", ""));
    let parts = match split(sep, "a,b,c") {
        Result::Ok(p) => p,
        Result::Err(_) => panic "split",
    };
    print "%s|%s|%s", parts[0], parts[1], parts[2];
}
