// Client-side pure helpers for the echo demo.
// TCP connect/send/recv stays in `main.hy` for layout clarity.

fn request_body() -> [byte] {
    let a: byte = 65;
    let b: byte = 66;
    let body: [byte] = [a];
    body[] = b;
    return body;
}

fn client_port() -> int {
    return 41235;
}
