// Recursive ADT walk without a proven int measure needs #[max_depth(N)].
enum Tree {
    Leaf,
    Node(int, Tree, Tree),
}

fn sum_tree(Tree t) -> int {
    return match t {
        Tree::Leaf => 0,
        Tree::Node(v, left, right) => v + sum_tree(left) + sum_tree(right),
    };
}

fn main() {
    let _ = sum_tree(Tree::Leaf());
}
