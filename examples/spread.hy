fn pair_sum(int a, int b) -> int {
    return a + b;
}

fn triple_sum(int a, int b, int c) -> int {
    return a + b + c;
}

fn main() {
    print "%i", pair_sum(...(1, 2));
    print "%i", triple_sum(...[10, 20, 30]);
}
