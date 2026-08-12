// HashSet — HashMap wrapper in its own module (class types import across modules).

use collections::map::{HashMap};

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
