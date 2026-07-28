// examples/derive_hash.hy — recursive `#[derive(Hash)]` + primitive Hash.
//
// Output: true,true,true,true

#[derive(Hash)]
enum Inner {
    A(int),
}

#[derive(Hash)]
enum Outer {
    Wrap(Inner),
    Label { name: string, flag: bool },
}

fn main() {
    print "%z,", 42.hash() == 42.hash();
    print "%z,", "hi".hash() == "hi".hash();
    print "%z,", Outer::Wrap(Inner::A(1)).hash() == Outer::Wrap(Inner::A(1)).hash();
    print "%z", Outer::Wrap(Inner::A(1)).hash() != Outer::Wrap(Inner::A(2)).hash();
}
