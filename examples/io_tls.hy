// TLS via `io::net::tls` (host rustls; Cargo feature `tls`).
//
// Client:
//   let s = connect("example.com", 443)?;
//   let s = enable(s, "example.com", { verify: true })?;
//   let s = disable(s)?;
//
// Server (after accept):
//   let s = encrypt(s, { cert_pem: cert, key_pem: key })?;
//   let s = decrypt(s)?;
//
// Handshake runs in the host; the handle is a normal `Stream`.
// Machine unit tests cover client enable and server encrypt round-trips
// against local sockets (no public network required).
//
// Smoke: enable on a non-TCP stream → Err (InvalidInput).
use io::*;
use io::net::tls::*;

fn main() {
    let path = "/tmp/coil_tls_smoke.bin";
    let s = open(path, "w")?;
    let msg = match enable(s, "127.0.0.1", { verify: false }) {
        Result::Ok(_) => "unexpected-ok",
        Result::Err(_) => "tls-ok",
    };
    close(s)?;
    print "%s", msg;
}
