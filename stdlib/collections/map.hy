// HashMap — separate chaining over parallel Vecs (no Default on K/V).

/// Open-addressed hash table with separate chaining (parallel Vec storage).
class HashMap<K, V> {
    heads: Vec<int>,
    keys: Vec<K>,
    vals: Vec<V>,
    next: Vec<int>,
    live: Vec<int>,
    len: int,
    cap: int,
}

impl HashMap<K, V> {
    /// Create a map with bucket count rounded up to the next power of two (min 8).
    static fn with_capacity(int cap) -> HashMap<K, V> {
        let n = 1;
        while n < cap {
            n = n + n;
        }
        if n < 8 {
            n = 8;
        }
        let heads: Vec<int> = Vec::new();
        let i = 0;
        while i < n {
            heads.push(0 - 1);
            i = i + 1;
        }
        let keys: Vec<K> = Vec::new();
        let vals: Vec<V> = Vec::new();
        let next: Vec<int> = Vec::new();
        let live: Vec<int> = Vec::new();
        return new HashMap(heads, keys, vals, next, live, 0, n);
    }

    /// Empty map with default capacity 8.
    static fn new() -> HashMap<K, V> {
        return HashMap::with_capacity(8);
    }

    /// Number of live key-value pairs.
    /// Number of live key-value pairs.
    fn size() -> int {
        return self.len;
    }

    fn is_empty() -> bool {
        return self.len == 0;
    }

    /// Bucket count (power of two).
    fn capacity() -> int {
        return self.cap;
    }

    /// Remove all entries without shrinking buckets.
    fn clear() {
        let i = 0;
        while i < self.cap {
            self.heads[i] = 0 - 1;
            i = i + 1;
        }
        let n = self.live.len();
        let j = 0;
        while j < n {
            self.live[j] = 0;
            j = j + 1;
        }
        self.len = 0;
    }
}

impl HashMap<K: Eq + Hash, V> {
    fn hash_of(K k) -> int {
        return k.hash();
    }

    fn bucket(K k) -> int {
        return self.hash_of(k) & (self.cap - 1);
    }

    fn find(K k) -> int {
        let h = self.bucket(k);
        let idx = self.heads[h];
        while idx >= 0 {
            if self.live[idx] == 1 {
                if self.keys[idx] == k {
                    return idx;
                }
            }
            idx = self.next[idx];
        }
        return 0 - 1;
    }

    fn grow() {
        let new_cap = self.cap + self.cap;
        if new_cap < 8 {
            new_cap = 8;
        }
        let heads: Vec<int> = Vec::new();
        let i = 0;
        while i < new_cap {
            heads.push(0 - 1);
            i = i + 1;
        }
        let keys: Vec<K> = Vec::new();
        let vals: Vec<V> = Vec::new();
        let next: Vec<int> = Vec::new();
        let live: Vec<int> = Vec::new();
        let old_n = self.keys.len();
        let j = 0;
        while j < old_n {
            if self.live[j] == 1 {
                let k = self.keys[j];
                let v = self.vals[j];
                let h = self.hash_of(k) & (new_cap - 1);
                let slot = keys.len();
                keys.push(k);
                vals.push(v);
                next.push(heads[h]);
                live.push(1);
                heads[h] = slot;
            }
            j = j + 1;
        }
        self.heads = heads;
        self.keys = keys;
        self.vals = vals;
        self.next = next;
        self.live = live;
        self.cap = new_cap;
    }

    /// Insert or replace. Returns `true` when a new key was inserted.
    fn insert(K k, V v) -> bool {
        let found = self.find(k);
        if found >= 0 {
            self.vals[found] = v;
            return false;
        }
        if (self.len + self.len) >= self.cap {
            self.grow();
        }
        let h = self.bucket(k);
        let slot = self.keys.len();
        self.keys.push(k);
        self.vals.push(v);
        self.next.push(self.heads[h]);
        self.live.push(1);
        self.heads[h] = slot;
        self.len = self.len + 1;
        return true;
    }

    /// True when `k` is present.
    fn contains(K k) -> bool {
        return self.find(k) >= 0;
    }

    /// Return the value for `k`, or `fallback` when absent.
    fn get_or(K k, V fallback) -> V {
        let found = self.find(k);
        if found >= 0 {
            return self.vals[found];
        }
        return fallback;
    }

    /// Remove `k`. Returns `true` when a live entry was removed.
    fn remove(K k) -> bool {
        let h = self.bucket(k);
        let idx = self.heads[h];
        let prev = 0 - 1;
        while idx >= 0 {
            if self.live[idx] == 1 {
                if self.keys[idx] == k {
                    if prev < 0 {
                        self.heads[h] = self.next[idx];
                    } else {
                        self.next[prev] = self.next[idx];
                    }
                    self.live[idx] = 0;
                    self.len = self.len - 1;
                    return true;
                }
            }
            prev = idx;
            idx = self.next[idx];
        }
        return false;
    }
}

// ---- HashSet (same module so the HashMap type is in scope) ----

/// Set of unique values backed by `HashMap<T, bool>`.
class HashSet<T> {
    inner: HashMap<T, bool>,
}

impl HashSet<T> {
    /// Create a set with bucket count rounded up to the next power of two (min 8).
    static fn with_capacity(int cap) -> HashSet<T> {
        return new HashSet(HashMap::with_capacity(cap));
    }

    /// Empty set with default capacity 8.
    static fn new() -> HashSet<T> {
        return HashSet::with_capacity(8);
    }

    /// Number of elements.
    fn size() -> int {
        return self.inner.len;
    }

    fn is_empty() -> bool {
        return self.inner.len == 0;
    }

    /// Remove all elements.
    fn clear() {
        self.inner.clear();
    }
}

impl HashSet<T: Eq + Hash> {
    /// Insert `x`; returns `true` when newly added.
    fn insert(T x) -> bool {
        return self.inner.insert(x, true);
    }

    /// True when `x` is present.
    fn contains(T x) -> bool {
        return self.inner.contains(x);
    }

    /// Remove `x`; returns `true` when an element was removed.
    fn remove(T x) -> bool {
        return self.inner.remove(x);
    }
}
