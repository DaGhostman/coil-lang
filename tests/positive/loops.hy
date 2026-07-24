// while / for / break / continue.
test("while counts up") {
    let i = 0;
    let sum = 0;
    while i < 5 {
        sum = sum + i;
        i = i + 1;
    }
    assert(sum == 10)?;
    assert(i == 5)?;
}

test("while never enters") {
    let x = 0;
    while false {
        x = 1;
    }
    assert(x == 0)?;
}

test("c-style for sum") {
    let sum = 0;
    for (let i = 0; i < 5; i = i + 1) {
        sum = sum + i;
    }
    assert(sum == 10)?;
}

test("for continue skips") {
    let sum = 0;
    for (let i = 0; i < 6; i = i + 1) {
        if i == 3 {
            continue;
        }
        sum = sum + i;
    }
    assert(sum == 12)?; // 0+1+2+4+5
}

test("for break exits early") {
    let sum = 0;
    for (let i = 0; i < 100; i = i + 1) {
        if i == 4 {
            break;
        }
        sum = sum + i;
    }
    assert(sum == 6)?; // 0+1+2+3
}

test("for continue and break together") {
    let sum = 0;
    for (let i = 0; i < 10; i = i + 1) {
        if i == 3 {
            continue;
        }
        if i == 7 {
            break;
        }
        sum = sum + i;
    }
    assert(sum == 18)?; // 0+1+2+4+5+6
}

test("postfix increment in loop") {
    let y = 0;
    let n = 0;
    while n < 3 {
        y = y + 1;
        n = n + 1;
    }
    assert(y == 3)?;
}

test("nested loops") {
    let total = 0;
    for (let i = 0; i < 3; i = i + 1) {
        for (let j = 0; j < 3; j = j + 1) {
            total = total + 1;
        }
    }
    assert(total == 9)?;
}
