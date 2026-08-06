// Collection helpers (userland): sort, reverse, range materialize.

/// Stable-ish bubble sort for `Ord` elements (new array; input unchanged).
fn sort<T: Ord>(Vec<T> arr) -> Vec<T> {
    let n = len(arr);
    let out: Vec<T> = Vec::new();
    let i = 0;
    while i < n {
        out.push(arr[i]);
        i = i + 1;
    }
    let a = 0;
    while a < n {
        let b = 0;
        while b + 1 < n {
            if out[b] > out[b + 1] {
                let tmp = out[b];
                out[b] = out[b + 1];
                out[b + 1] = tmp;
            }
            b = b + 1;
        }
        a = a + 1;
    }
    return out;
}

/// Reverse a copy of `arr`.
fn reverse<T>(Vec<T> arr) -> Vec<T> {
    let n = len(arr);
    let out: Vec<T> = Vec::new();
    let i = n;
    while i > 0 {
        i = i - 1;
        out.push(arr[i]);
    }
    return out;
}

/// Materialize a lazy `Range<int>` into a dynamic array.
fn collect_ints(Range<int> r) -> Vec<int> {
    let out: Vec<int> = Vec::new();
    for x in r {
        out.push(x);
    }
    return out;
}

/// Materialize a lazy inclusive `RangeInclusive<int>`.
fn collect_ints_inclusive(RangeInclusive<int> r) -> Vec<int> {
    let out: Vec<int> = Vec::new();
    for x in r {
        out.push(x);
    }
    return out;
}
