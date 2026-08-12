// 04-http — coil-http Client against local Server.
//
// Expected output: ok
use http::client::{Client};

use io::{stdout};
use io::sync::{write_all};
use string::{to_bytes};

fn main() {
    let label = "err";
    let client = Client::new();
    match client.get("http://127.0.0.1:41250/") {
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
