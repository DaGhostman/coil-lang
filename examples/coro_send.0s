async fn ping() {
    let msg = yield "ready";
    print "%s", msg;
}

fn main() {
    let h = ping();
    resume h;
    resume h with "hello";
}
