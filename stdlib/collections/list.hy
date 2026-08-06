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
    fn size() -> int {
        return self.len;
    }

    fn is_empty() -> bool {
        return self.len == 0;
    }
}

fn list_new<T>() -> List<T> {
    return new List(Option::None, 0);
}

fn list_push_front<T>(List<T> xs, T v) {
    let n = new Node(v, xs.head);
    xs.head = Option::Some(n);
    xs.len = xs.len + 1;
}

fn list_peek_front_or<T>(List<T> xs, T fallback) -> T {
    return match xs.head {
        Option::None => fallback,
        Option::Some(n) => n.value,
    };
}

fn list_pop_front_or<T>(List<T> xs, T fallback) -> T {
    return match xs.head {
        Option::None => fallback,
        Option::Some(n) => {
            xs.head = n.next;
            xs.len = xs.len - 1;
            return n.value;
        },
    };
}

fn list_clear<T>(List<T> xs) {
    xs.head = Option::None;
    xs.len = 0;
}
