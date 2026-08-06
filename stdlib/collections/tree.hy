// Mutable BST over Ord+Eq keys, index-linked nodes (no Option field matches).

/// Mutable binary search tree keyed by `Ord`+`Eq`, stored as parallel Vecs.
class TreeMap<K, V> {
    keys: Vec<K>,
    vals: Vec<V>,
    left: Vec<int>,
    right: Vec<int>,
    root: int,
    len: int,
}

impl TreeMap<K, V> {
    /// Empty map with default storage.
    static fn new() -> TreeMap<K, V> {
        let keys: Vec<K> = Vec::new();
        let vals: Vec<V> = Vec::new();
        let left: Vec<int> = Vec::new();
        let right: Vec<int> = Vec::new();
        return new TreeMap(keys, vals, left, right, 0 - 1, 0);
    }

    /// Alias for `new`.
    static fn empty() -> TreeMap<K, V> {
        return TreeMap::new();
    }

    /// Number of key-value pairs.
    fn size() -> int {
        return self.len;
    }

    fn is_empty() -> bool {
        return self.len == 0;
    }
}

impl TreeMap<K: Ord + Eq, V> {
    /// Insert or update; returns `true` when a new key was added.
    fn insert(K k, V v) -> bool {
        if self.root < 0 {
            let slot = self.keys.len();
            self.keys.push(k);
            self.vals.push(v);
            self.left.push(0 - 1);
            self.right.push(0 - 1);
            self.root = slot;
            self.len = 1;
            return true;
        }
        let cur = self.root;
        while true {
            if k == self.keys[cur] {
                self.vals[cur] = v;
                return false;
            }
            if k < self.keys[cur] {
                let child = self.left[cur];
                if child < 0 {
                    let slot = self.keys.len();
                    self.keys.push(k);
                    self.vals.push(v);
                    self.left.push(0 - 1);
                    self.right.push(0 - 1);
                    self.left[cur] = slot;
                    self.len = self.len + 1;
                    return true;
                }
                cur = child;
            } else {
                let child = self.right[cur];
                if child < 0 {
                    let slot = self.keys.len();
                    self.keys.push(k);
                    self.vals.push(v);
                    self.left.push(0 - 1);
                    self.right.push(0 - 1);
                    self.right[cur] = slot;
                    self.len = self.len + 1;
                    return true;
                }
                cur = child;
            }
        }
    }

    /// True when `k` is present.
    fn contains(K k) -> bool {
        let cur = self.root;
        while cur >= 0 {
            if k == self.keys[cur] {
                return true;
            }
            if k < self.keys[cur] {
                cur = self.left[cur];
            } else {
                cur = self.right[cur];
            }
        }
        return false;
    }

    /// Value for `k`, or `fallback` when absent.
    fn get_or(K k, V fallback) -> V {
        let cur = self.root;
        while cur >= 0 {
            if k == self.keys[cur] {
                return self.vals[cur];
            }
            if k < self.keys[cur] {
                cur = self.left[cur];
            } else {
                cur = self.right[cur];
            }
        }
        return fallback;
    }
}
