// Mutable singly-linked list.

class Node<T> {
    value: T,
    next: Option<Node<T>>,
}

class List<T> {
    head: Option<Node<T>>,
    len: int,
}

impl List<T> {
    static fn new() -> List<T> {
        return new List(Option::None, 0);
    }

    fn size() -> int {
        return self.len;
    }

    fn is_empty() -> bool {
        return self.len == 0;
    }

    fn push_front(T v) {
        let n = new Node(v, self.head);
        self.head = Option::Some(n);
        self.len = self.len + 1;
    }

    fn peek_front_or(T fallback) -> T {
        return match self.head {
            Option::None => fallback,
            Option::Some(n) => n.value,
        };
    }

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

    fn clear() {
        self.head = Option::None;
        self.len = 0;
    }
}
