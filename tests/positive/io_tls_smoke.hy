// HostInvoke + typechecker smoke for `io::net::tls::client` (feature `tls`).
// enable on a file stream → Err (no network / certificates needed).
use io::*;
use io::net::tls::client::*;

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
