// HostInvoke + typechecker smoke for `io::net::tls::{client,server}` (feature `tls`).
// Kind / opts errors only — no network / certificates needed.
use io::*;
use io::net::tls::client::enable as client_enable;
use io::net::tls::client::disable as client_disable;
use io::net::tls::server::enable as server_enable;
use io::net::tls::server::disable as server_disable;

test("tls client enable on non-TCP stream is Err") {
    let path = "/tmp/coil_tls_harness.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match client_enable(s, "127.0.0.1", { verify: false, ca_pem: Option::None, ca_path: Option::None, timeout_ms: 0 }) {
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
        Result::Ok(s) => match server_enable(s, { cert_pem: "x", key_pem: "y", timeout_ms: 0, client_ca_pem: "" }) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}

test("tls client disable on non-TLS stream is Err") {
    let path = "/tmp/coil_tls_harness_disable.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match client_disable(s) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}

test("tls server enable empty PEM on non-TCP is Err") {
    let path = "/tmp/coil_tls_harness_server_pem.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match server_enable(s, { cert_pem: "", key_pem: "", timeout_ms: 0, client_ca_pem: "" }) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}

test("tls server disable on non-TLS stream is Err") {
    let path = "/tmp/coil_tls_harness_server_disable.bin";
    let ok = match open(path, "w") {
        Result::Ok(s) => match server_disable(s) {
            Result::Ok(_) => 0,
            Result::Err(_) => 1,
        },
        Result::Err(_) => 9,
    };
    assert(ok == 1)?;
}
