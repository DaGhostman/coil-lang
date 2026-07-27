// `defer` runs on function exit (return or fall-through), LIFO.
// Outer locals must be listed in `use (…)` — same capture rule as lambdas.
fn with_cleanup() {
    defer {
        print "leave";
    }
    print "enter";
}

fn lifo() {
    defer {
        print "1";
    }
    defer {
        print "2";
    }
    print "0";
}

// Early `return` still runs deferred cleanup.
fn early_return(int n) -> int {
    defer {
        print "d";
    }
    if n == 0 {
        return 99;
    }
    print "ok";
    return n;
}

// Capture an outer local with `defer use (n)`.
fn capture_n(int n) -> int {
    defer use (n) {
        print "%i", n;
    }
    return n;
}

fn main() {
    with_cleanup();
    print ",";
    lifo();
    print ",";
    print "%i", early_return(7);
    print ",";
    print "%i", early_return(0);
    print ",";
    print "%i", capture_n(5);
}
