// HostInvoke + typechecker smoke for `io::net::tls::{client,server}` (feature `tls`).
// enable on a file stream → Err (no network / certificates needed).
use io::*;
use io::net::tls::client::*;
use io::net::tls::server::enable as server_enable;

test("tls client enable on non-TCP stream is Err") {
    let path = "/tmp/coil_tls_harness.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match enable(s, "127.0.0.1", { verify: false }) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}

test("tls server enable on non-TCP stream is Err") {
    let path = "/tmp/coil_tls_harness_server.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match server_enable(s, { cert_pem: "x", key_pem: "y" }) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}
