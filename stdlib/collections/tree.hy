// Mutable BST over Ord+Eq keys, index-linked nodes (no Option field matches).

class TreeMap<K, V> {
    keys: [K],
    vals: [V],
    left: [int],
    right: [int],
    root: int,
    len: int,
}

impl TreeMap<K, V> {
    fn size() -> int {
        return self.len;
    }

    fn is_empty() -> bool {
        return self.len == 0;
    }
}

fn treemap_new<K, V>() -> TreeMap<K, V> {
    let keys: [K] = [];
    let vals: [V] = [];
    let left: [int] = [];
    let right: [int] = [];
    return new TreeMap(keys, vals, left, right, 0 - 1, 0);
}

fn treemap_empty<K, V>() -> TreeMap<K, V> {
    return treemap_new();
}

fn treemap_insert<K: Ord + Eq, V>(TreeMap<K, V> t, K k, V v) -> bool {
    if t.root < 0 {
        let slot = len(t.keys);
        t.keys[] = k;
        t.vals[] = v;
        t.left[] = 0 - 1;
        t.right[] = 0 - 1;
        t.root = slot;
        t.len = 1;
        return true;
    }
    let cur = t.root;
    let steps = 0;
    while steps < 100000 {
        steps = steps + 1;
        if k == t.keys[cur] {
            t.vals[cur] = v;
            return false;
        }
        if k < t.keys[cur] {
            let child = t.left[cur];
            if child < 0 {
                let slot = len(t.keys);
                t.keys[] = k;
                t.vals[] = v;
                t.left[] = 0 - 1;
                t.right[] = 0 - 1;
                t.left[cur] = slot;
                t.len = t.len + 1;
                return true;
            }
            cur = child;
        } else {
            let child = t.right[cur];
            if child < 0 {
                let slot = len(t.keys);
                t.keys[] = k;
                t.vals[] = v;
                t.left[] = 0 - 1;
                t.right[] = 0 - 1;
                t.right[cur] = slot;
                t.len = t.len + 1;
                return true;
            }
            cur = child;
        }
    }
    return false;
}

fn treemap_contains<K: Ord + Eq, V>(TreeMap<K, V> t, K k) -> bool {
    let cur = t.root;
    let steps = 0;
    while cur >= 0 {
        if steps >= 100000 {
            return false;
        }
        steps = steps + 1;
        if k == t.keys[cur] {
            return true;
        }
        if k < t.keys[cur] {
            cur = t.left[cur];
        } else {
            cur = t.right[cur];
        }
    }
    return false;
}

fn treemap_get_or<K: Ord + Eq, V>(TreeMap<K, V> t, K k, V fallback) -> V {
    let cur = t.root;
    let steps = 0;
    while cur >= 0 {
        if steps >= 100000 {
            return fallback;
        }
        steps = steps + 1;
        if k == t.keys[cur] {
            return t.vals[cur];
        }
        if k < t.keys[cur] {
            cur = t.left[cur];
        } else {
            cur = t.right[cur];
        }
    }
    return fallback;
}
