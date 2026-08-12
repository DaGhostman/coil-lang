extern "c" {
    fn system(string cmd) -> int;
}

fn run_twice() -> int {
    let a = system("true");
    let v = Vec::new();
    v.push("x");
    let b = system("true");
    return a + b;
}
