// Mutable singly-linked list.

class Node<T> {
    value: T,
    next: Option<Node<T>>,
}

/// Mutable singly-linked list with O(1) push/pop at the front.
class List<T> {
    head: Option<Node<T>>,
    len: int,
}

impl List<T> {
    /// Empty list.
    static fn new() -> List<T> {
        return new List(Option::None, 0);
    }

    /// Number of elements.
    fn size() -> int {
        return self.len;
    }

    fn is_empty() -> bool {
        return self.len == 0;
    }

    /// Prepend `v`.
    fn push_front(T v) {
        let n = new Node(v, self.head);
        self.head = Option::Some(n);
        self.len = self.len + 1;
    }

    /// Front element, or `fallback` when empty.
    fn peek_front_or(T fallback) -> T {
        return match self.head {
            Option::None => fallback,
            Option::Some(n) => n.value,
        };
    }

    /// Remove and return front, or `fallback` when empty.
    fn pop_front_or(T fallback) -> T {
        return match self.head {
            Option::None => fallback,
            Option::Some(n) => {
                self.head = n.next;
                self.len = self.len - 1;
                return n.value;
            },
        };
    }

    /// Drop all elements.
    fn clear() {
        self.head = Option::None;
        self.len = 0;
    }
}
