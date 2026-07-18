// Binary higher-kinded trait: Bifunctor<F: * -> * -> *>.
// Expected output: 42

trait Bifunctor<F: * -> * -> *> {
    fn tag<A, B>(F<A, B> xs) -> int;
}

impl Bifunctor<Result> {
    fn tag<A, B>(Result<A, B> xs) -> int {
        return 42;
    }
}

fn get_tag<F: * -> * -> *, Bifunctor, A, B>(F<A, B> xs) -> int {
    return tag(xs);
}

fn main() {
    print "%i", get_tag(Result::Ok(7));
}
