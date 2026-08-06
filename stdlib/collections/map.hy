// HashMap — separate chaining over parallel arrays (no Default on K/V).
//
// Constrained ops are free functions: inherent `impl` bounds are not yet
// applied to method schemes (see docs/internals/collections-vm-split.md).

class HashMap<K, V> {
    heads: [int],
    keys: [K],
    vals: [V],
    next: [int],
    live: [int],
    len: int,
    cap: int,
}

impl HashMap<K, V> {
    fn size() -> int {
        return self.len;
    }

    fn is_empty() -> bool {
        return self.len == 0;
    }

    fn capacity() -> int {
        return self.cap;
    }
}

fn hashmap_with_capacity<K, V>(int cap) -> HashMap<K, V> {
    let n = 1;
    while n < cap {
        n = n + n;
    }
    if n < 8 {
        n = 8;
    }
    let heads: [int] = [];
    let i = 0;
    while i < n {
        heads[] = 0 - 1;
        i = i + 1;
    }
    let keys: [K] = [];
    let vals: [V] = [];
    let next: [int] = [];
    let live: [int] = [];
    return new HashMap(heads, keys, vals, next, live, 0, n);
}

fn hashmap_new<K, V>() -> HashMap<K, V> {
    return hashmap_with_capacity(8);
}

fn hashmap_hash_of<K: Hash>(K k) -> int {
    return k.hash();
}

fn hashmap_bucket<K: Hash, V>(HashMap<K, V> m, K k) -> int {
    return hashmap_hash_of(k) & (m.cap - 1);
}

fn hashmap_find<K: Eq + Hash, V>(HashMap<K, V> m, K k) -> int {
    let h = hashmap_bucket(m, k);
    let idx = m.heads[h];
    while idx >= 0 {
        if m.live[idx] == 1 {
            if m.keys[idx] == k {
                return idx;
            }
        }
        idx = m.next[idx];
    }
    return 0 - 1;
}

fn hashmap_grow<K: Eq + Hash, V>(HashMap<K, V> m) {
    let new_cap = m.cap + m.cap;
    if new_cap < 8 {
        new_cap = 8;
    }
    let heads: [int] = [];
    let i = 0;
    while i < new_cap {
        heads[] = 0 - 1;
        i = i + 1;
    }
    let keys: [K] = [];
    let vals: [V] = [];
    let next: [int] = [];
    let live: [int] = [];
    let old_n = len(m.keys);
    let j = 0;
    while j < old_n {
        if m.live[j] == 1 {
            let k = m.keys[j];
            let v = m.vals[j];
            let h = hashmap_hash_of(k) & (new_cap - 1);
            let slot = len(keys);
            keys[] = k;
            vals[] = v;
            next[] = heads[h];
            live[] = 1;
            heads[h] = slot;
        }
        j = j + 1;
    }
    m.heads = heads;
    m.keys = keys;
    m.vals = vals;
    m.next = next;
    m.live = live;
    m.cap = new_cap;
}

/// Insert or replace. Returns `true` when a new key was inserted.
fn hashmap_insert<K: Eq + Hash, V>(HashMap<K, V> m, K k, V v) -> bool {
    let found = hashmap_find(m, k);
    if found >= 0 {
        m.vals[found] = v;
        return false;
    }
    if (m.len + m.len) >= m.cap {
        hashmap_grow(m);
    }
    let h = hashmap_bucket(m, k);
    let slot = len(m.keys);
    m.keys[] = k;
    m.vals[] = v;
    m.next[] = m.heads[h];
    m.live[] = 1;
    m.heads[h] = slot;
    m.len = m.len + 1;
    return true;
}

fn hashmap_contains<K: Eq + Hash, V>(HashMap<K, V> m, K k) -> bool {
    return hashmap_find(m, k) >= 0;
}

/// Return the value for `k`, or `fallback` when absent.
fn hashmap_get_or<K: Eq + Hash, V>(HashMap<K, V> m, K k, V fallback) -> V {
    let found = hashmap_find(m, k);
    if found >= 0 {
        return m.vals[found];
    }
    return fallback;
}

/// Remove `k`. Returns `true` when a live entry was removed.
fn hashmap_remove<K: Eq + Hash, V>(HashMap<K, V> m, K k) -> bool {
    let h = hashmap_bucket(m, k);
    let idx = m.heads[h];
    let prev = 0 - 1;
    while idx >= 0 {
        if m.live[idx] == 1 {
            if m.keys[idx] == k {
                if prev < 0 {
                    m.heads[h] = m.next[idx];
                } else {
                    m.next[prev] = m.next[idx];
                }
                m.live[idx] = 0;
                m.len = m.len - 1;
                return true;
            }
        }
        prev = idx;
        idx = m.next[idx];
    }
    return false;
}

fn hashmap_clear<K, V>(HashMap<K, V> m) {
    let i = 0;
    while i < m.cap {
        m.heads[i] = 0 - 1;
        i = i + 1;
    }
    let n = len(m.live);
    let j = 0;
    while j < n {
        m.live[j] = 0;
        j = j + 1;
    }
    m.len = 0;
}

// ---- HashSet (same module so the HashMap type is in scope) ----

class HashSet<T> {
    inner: HashMap<T, bool>,
}

impl HashSet<T> {
    fn size() -> int {
        return self.inner.len;
    }

    fn is_empty() -> bool {
        return self.inner.len == 0;
    }
}

fn hashset_with_capacity<T>(int cap) -> HashSet<T> {
    return new HashSet(hashmap_with_capacity(cap));
}

fn hashset_new<T>() -> HashSet<T> {
    return hashset_with_capacity(8);
}

fn hashset_insert<T: Eq + Hash>(HashSet<T> s, T x) -> bool {
    return hashmap_insert(s.inner, x, true);
}

fn hashset_contains<T: Eq + Hash>(HashSet<T> s, T x) -> bool {
    return hashmap_contains(s.inner, x);
}

fn hashset_remove<T: Eq + Hash>(HashSet<T> s, T x) -> bool {
    return hashmap_remove(s.inner, x);
}

fn hashset_clear<T>(HashSet<T> s) {
    hashmap_clear(s.inner);
}
