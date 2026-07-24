// Expected output: 7424.0427
//
// Userland generic functions with arithmetic trait bounds.
// `Num` is a convenience supertrait of `Add`/`Sub`/`Mul`/`Div`.
// Callers that only need `+` can bound `T: Add` instead.
// See typeclass_dict.hy and polyfn.hy for user dictionaries and PolyFn values.

fn add<T: Num>(T a, T b) -> T {
    return a + b;
}

fn just_add<T: Add>(T a, T b) -> T {
    return a + b;
}

fn main() {
    print "%i", add(3, 4);
    print "%i", add(10, 32);
    print "%f", add(1.5, 2.5);
    // Escaping through PolyFn forces the shared boxed/dictionary path.
    let add_value = add;
    print "%i", add_value(20, 22);
    print "%i", just_add(3, 4);
}
