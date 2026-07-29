// HostInvoke + typechecker smoke for `io::net::tls` (feature `tls`).
// Kind / opts errors only — no network / certificates needed.
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

test("tls encrypt empty PEM on non-TCP is Err") {
    let path = "/tmp/coil_tls_harness_encrypt.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match encrypt(s, { cert_pem: "", key_pem: "" }) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}

test("tls decrypt on non-TLS stream is Err") {
    let path = "/tmp/coil_tls_harness_decrypt.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match decrypt(s) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}
