attr log<T>(fn(...args) -> T target, string message, ...args) -> T {
    print "%s", message;
    return target(...args);
}

attr measure<T>(fn(...args) -> T target, string metric, ...args) -> T {
    print "%s", metric;
    return target(...args);
}

#[log(message = "enter")]
#[measure(metric = "do_thing")]
fn do_thing(int x, string name) -> int {
    print "%s", name;
    return x;
}

fn main() {
    print "%i", do_thing(42, "hi");
}
