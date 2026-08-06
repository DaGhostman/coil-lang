// Collection helpers (userland): sort, reverse, range materialize.

/// Merge `buf[lo..mid)` and `buf[mid..hi)` via `tmp` scratch (stable).
fn merge_range<T: Ord>([T] buf, [T] tmp, int lo, int mid, int hi) -> int {
    let i = lo;
    while i < hi {
        tmp[i] = buf[i];
        i = i + 1;
    }
    let a = lo;
    let b = mid;
    let k = lo;
    while a < mid {
        if b < hi {
            if tmp[b] < tmp[a] {
                buf[k] = tmp[b];
                b = b + 1;
            } else {
                buf[k] = tmp[a];
                a = a + 1;
            }
            k = k + 1;
        } else {
            break;
        }
    }
    while a < mid {
        buf[k] = tmp[a];
        a = a + 1;
        k = k + 1;
    }
    while b < hi {
        buf[k] = tmp[b];
        b = b + 1;
        k = k + 1;
    }
    return 0;
}

fn min_int(int a, int b) -> int {
    if a < b {
        return a;
    }
    return b;
}

/// Stable bottom-up mergesort for `Ord` elements (new array; input unchanged).
fn sort<T: Ord>([T] arr) -> [T] {
    let n = len(arr);
    let out: [T] = [];
    let tmp: [T] = [];
    let copy_i = 0;
    while copy_i < n {
        out[] = arr[copy_i];
        tmp[] = arr[copy_i];
        copy_i = copy_i + 1;
    }
    let width = 1;
    while width < n {
        let pos = 0;
        while pos < n {
            let lo = pos;
            let mid = min_int(pos + width, n);
            let hi = min_int(pos + width + width, n);
            if mid < hi {
                merge_range(out, tmp, lo, mid, hi);
            }
            pos = pos + width + width;
        }
        width = width + width;
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
