// Function calls, recursion, early return, multiple params.
fn add(int a, int b) -> int {
    return a + b;
}

fn fib(int n) -> int {
    if n <= 2 {
        return 1;
    }
    return fib(n - 1) + fib(n - 2);
}

fn early(int n, int is_neg) -> int {
    if is_neg == 1 {
        return 0 - 1;
    }
    return n * 2;
}

fn multi(int a, int b, int c) -> int {
    return a + b * c;
}

test("basic call") {
    assert(add(2, 3) == 5)?;
}

test("recursion fib") {
    assert(fib(1) == 1)?;
    assert(fib(2) == 1)?;
    assert(fib(6) == 8)?;
    assert(fib(10) == 55)?;
}

test("early return") {
    assert(early(4, 1) == 0 - 1)?;
    assert(early(4, 0) == 8)?;
}

test("multi arg precedence in callee") {
    assert(multi(1, 2, 3) == 7)?;
}

test("nested calls") {
    assert(add(add(1, 2), add(3, 4)) == 10)?;
}
