// 04-http — stdlib HTTP/1.1 client against a local cleartext server.
//
// Expected output: ok
use http::client::*;
use io::{stdout, write_all};
use string::{format, to_bytes};

fn main() {
    let label = "err";
    match get("http://127.0.0.1:41250/") {
        Result::Ok(_) => {
            label = "ok";
        },
        Result::Err(_) => {
            label = "err";
        },
    };
    if label != "ok" {
        panic "get failed";
    }
    write_all(stdout(), to_bytes("ok"));
}
