// UDP datagram round-trip via `io::net::udp`.
// Server binds ephemeral port; client send_to; server recv_from_wait.
use io::{close, stdout};
use io::net::udp::{bind, local_port, send_to};
use io::sync::{recv_from_wait, write_all};

use string::{format, to_bytes};

fn echo_once() {
    let server = bind("127.0.0.1", 0)?;
    let port = local_port(server)?;
    let client = bind("127.0.0.1", 0)?;
    let msg: [byte] = [72, 105]; // "Hi"
    send_to(client, msg, "127.0.0.1", port)?;
    let buf: [byte] = [0, 0, 0, 0, 0, 0, 0, 0];
    let t = recv_from_wait(server, buf)?;
    close(server)?;
    close(client)?;
    return format("%i", t[0]);
}

fn main() {
    write_all(stdout(), to_bytes(format("%s", match echo_once() {
        Result::Ok(s) => s,
        Result::Err(_) => "err",
    })));
}
