// HostInvoke + typechecker smoke for `io::net::tls` (feature `tls`).
// enable on a file stream → Err; disable on non-TLS → Err (no network).
use io::*;
use io::net::tls::*;

test("tls enable on non-TCP stream is Err") {
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

test("tls disable on non-TLS stream is Err") {
    let path = "/tmp/coil_tls_harness_disable.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match disable(s) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}
