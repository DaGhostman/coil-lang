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
    static fn new() -> TreeMap<K, V> {
        let keys: [K] = [];
        let vals: [V] = [];
        let left: [int] = [];
        let right: [int] = [];
        return new TreeMap(keys, vals, left, right, 0 - 1, 0);
    }

    static fn empty() -> TreeMap<K, V> {
        return TreeMap::new();
    }

    fn size() -> int {
        return self.len;
    }

    fn is_empty() -> bool {
        return self.len == 0;
    }
}

impl TreeMap<K: Ord + Eq, V> {
    fn insert(K k, V v) -> bool {
        if self.root < 0 {
            let slot = len(self.keys);
            self.keys[] = k;
            self.vals[] = v;
            self.left[] = 0 - 1;
            self.right[] = 0 - 1;
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
                    let slot = len(self.keys);
                    self.keys[] = k;
                    self.vals[] = v;
                    self.left[] = 0 - 1;
                    self.right[] = 0 - 1;
                    self.left[cur] = slot;
                    self.len = self.len + 1;
                    return true;
                }
                cur = child;
            } else {
                let child = self.right[cur];
                if child < 0 {
                    let slot = len(self.keys);
                    self.keys[] = k;
                    self.vals[] = v;
                    self.left[] = 0 - 1;
                    self.right[] = 0 - 1;
                    self.right[cur] = slot;
                    self.len = self.len + 1;
                    return true;
                }
                cur = child;
            }
        }
    }

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
