// TLS client via `io::net::tls` (host rustls; Cargo feature `tls`).
//
//   let s = connect("example.com", 443)?;
//   let s = enable(s, "example.com", { verify: true })?;   // webpki roots + SNI
//   let s = enable(s, "127.0.0.1", { verify: false })?;  // no cert trust (dev)
//   // then write_all / read / read_exact / read_to_end / close
//   let s = disable(s)?;  // plaintext on same fd
//
// Handshake runs in the host; the handle is a normal `Stream`.
// Machine unit tests cover `enable(..., { verify: false })` against a local
// rustls echo server (no public network required).
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
