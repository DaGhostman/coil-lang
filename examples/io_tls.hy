// TLS via `io::net::tls::{client,server}` (host rustls; Cargo feature `tls`).
//
// Client:
//   use io::net::tls::client::*;
//   let s = connect("example.com", 443)?;
//   let s = enable(s, "example.com", { verify: true, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 })?;
//   let s = disable(s)?;
//
// Server (after accept):
//   use io::net::tls::server::*;
//   let s = enable(s, { cert_pem: cert, key_pem: key, timeout_ms: 0, client_ca_pem: "" })?;
//   let s = disable(s)?;
//
// Handshake runs in the host; the handle is a normal `Stream`.
// Machine unit tests cover client enable and server enable round-trips
// against local sockets (no public network required).
//
// Smoke: client enable on a non-TCP stream → Err (InvalidInput).
use io::*;
use io::net::tls::client::*;

fn main() {
    let path = "/tmp/coil_tls_smoke.bin";
    let s = open(path, "w")?;
    let msg = match enable(s, "127.0.0.1", { verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 }) {
        Result::Ok(_) => "unexpected-ok",
        Result::Err(_) => "tls-ok",
    };
    close(s)?;
    print "%s", msg;
}
