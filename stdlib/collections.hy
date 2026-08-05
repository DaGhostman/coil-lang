// Collection helpers (userland): sort, reverse, range materialize.

/// Stable-ish bubble sort for `Ord` elements (new array; input unchanged).
fn sort<T: Ord>([T] arr) -> [T] {
    let n = len(arr);
    let out: [T] = [];
    let i = 0;
    while i < n {
        out[] = arr[i];
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
fn reverse<T>([T] arr) -> [T] {
    let n = len(arr);
    let out: [T] = [];
    let i = n;
    while i > 0 {
        i = i - 1;
        out[] = arr[i];
    }
    return out;
}

/// Materialize a lazy `Range<int>` into a dynamic array.
fn collect_ints(Range<int> r) -> [int] {
    let out: [int] = [];
    for x in r {
        out[] = x;
    }
    return out;
}

/// Materialize a lazy inclusive `RangeInclusive<int>`.
fn collect_ints_inclusive(RangeInclusive<int> r) -> [int] {
    let out: [int] = [];
    for x in r {
        out[] = x;
    }
    return out;
}
