// HostInvoke + typechecker smoke for `io::net::tls` (feature `tls`).
// Connects to a closed local port so CI needs no network / certificates.
use io::*;
use io::net::tls::*;

test("tls connect_insecure to closed port is Err") {
    let ok = match connect_insecure("127.0.0.1", 1) {
        Result::Ok(_) => 0,
        Result::Err(_) => 1,
    };
    assert(ok == 1)?;
}
