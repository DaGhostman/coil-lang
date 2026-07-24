// examples/tree.hy — recursive enum to verify isorecursive encoding
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
    print "%i", sum_tree(Tree::Node(1,
                Tree::Node(2, Tree::Leaf(), Tree::Leaf()),
                Tree::Node(3, Tree::Leaf(), Tree::Leaf())));
}