// Client-side pure helpers for the echo demo.
// TCP connect/send/recv stays in `main.hy` for layout clarity.

fn request_body() -> Vec<byte> {
    let a: byte = 65;
    let b: byte = 66;
    let body: Vec<byte> = Vec::new();
    body.push(a);
    body.push(b);
    return body;
}

fn client_port() -> int {
    return 41235;
}
