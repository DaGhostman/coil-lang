// PCRE2 virtual module: compile flags, find_all, split, replace_all.
use regex::*;
use io::{stdout};
use io::sync::{write_all};
use string::{format, to_bytes};

fn must_re(Result r) -> Regex {
    return match r {
        Result::Ok(v) => v,
        Result::Err(_) => panic "regex compile failed",
    };
}

fn main() {
    let re = must_re(compile("(\\w+)=(\\d+)", "i"));
    write_all(stdout(), to_bytes(format("%z,", match is_match(re, "X=42") {
        Result::Ok(b) => b,
        Result::Err(_) => false,
    })));
    let spans = match find_all(re, "a=1 b=2") {
        Result::Ok(s) => s,
        Result::Err(_) => panic "find_all",
    };
    write_all(stdout(), to_bytes(format("%i,", len(spans))));
    write_all(stdout(), to_bytes(format("%s,", match replace_all(re, "a=1 b=2", "$1->$2") {
        Result::Ok(s) => s,
        Result::Err(_) => "",
    })));
    let sep = must_re(compile(",", ""));
    let parts = match split(sep, "a,b,c") {
        Result::Ok(p) => p,
        Result::Err(_) => panic "split",
    };
    write_all(stdout(), to_bytes(format("%s|%s|%s", parts[0], parts[1], parts[2])));
}
