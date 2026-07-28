// TLS client via `io::net::tls` (host rustls; Cargo feature `tls`).
//
//   let s = connect("example.com", 443)?;           // webpki roots + SNI
//   let s = connect_insecure("127.0.0.1", 8443)?;  // no cert verify
//   // then write_all / read / read_exact / read_to_end / close
//
// Handshake runs in the host; the handle is a normal `Stream`.
// Machine unit tests cover `connect_insecure` against a local rustls
// echo server (no public network required).
//
// Smoke: call `connect_insecure` to a closed local port (expect Err).
use io::*;
use io::net::tls::*;

fn main() {
    let msg = match connect_insecure("127.0.0.1", 1) {
        Result::Ok(s) => {
            close(s)?;
            "unexpected-ok"
        },
        Result::Err(_) => "tls-ok",
    };
    print "%s", msg;
}
