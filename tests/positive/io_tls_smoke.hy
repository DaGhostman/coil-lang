// HostInvoke + typechecker smoke for `io::net::tls` (feature `tls`).
// Connects to a closed local port so CI needs no network / certificates.
use io::*;
use io::net::tls::*;

test("tls connect_insecure to closed port is Err") {
    let msg = match connect_insecure("127.0.0.1", 1) {
        Result::Ok(s) => {
            close(s)?;
            "unexpected-ok"
        },
        Result::Err(_) => "tls-ok",
    };
    assert(msg == "tls-ok")?;
}
