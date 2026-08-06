// Tiny cleartext HTTP/1.1 server for the 04-http demo.
// Serves one fixed response then exits.
// Reads the full request head (until CRLFCRLF) before responding so the
// client can finish write_all without a connection reset.
use io::{Stream, close, read};
use io::net::tcp::{listen};
use string::{to_bytes};
use io::sync::{accept_wait, write_all};

fn fixed_response() -> [byte] {
    return to_bytes("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nok");
}

fn find_header_end([byte] buf) -> int {
    let cr: byte = 13;
    let lf: byte = 10;
    let n = len(buf);
    let i = 0;
    while i + 3 < n {
        if buf[i] == cr {
            if buf[i + 1] == lf {
                if buf[i + 2] == cr {
                    if buf[i + 3] == lf {
                        return i + 4;
                    }
                }
            }
        }
        i = i + 1;
    }
    return 999999;
}

fn drain_request(Stream server) {
    let acc: [byte] = [];
    let chunk: [byte] = [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0];
    let guard = 0;
    let done = 0;
    while done == 0 {
        if guard >= 64 {
            done = 1;
        }
        if done == 0 {
            let end = find_header_end(acc);
            if end != 999999 {
                done = 1;
            }
        }
        if done == 0 {
            let nopt = match read(server, chunk) {
                Result::Ok(o) => o,
                Result::Err(_) => Option::None,
            };
            let nread = 0;
            let got = 0;
            match nopt {
                Option::None => {
                    done = 1;
                },
                Option::Some(n) => {
                    nread = n;
                    got = 1;
                },
            };
            if got == 1 {
                let j = 0;
                while j < nread {
                    acc[] = chunk[j];
                    j = j + 1;
                }
            }
        }
        guard = guard + 1;
    }
}

fn main() {
    let port = 41250;
    let listener = match listen("127.0.0.1", port) {
        Result::Ok(s) => s,
        Result::Err(_) => panic "listen",
    };
    let server = match accept_wait(listener) {
        Result::Ok(s) => s,
        Result::Err(_) => panic "accept",
    };
    drain_request(server);
    let msg = fixed_response();
    match write_all(server, msg) {
        Result::Ok(_) => 0,
        Result::Err(_) => panic "write",
    };
    match close(server) {
        Result::Ok(_) => 0,
        Result::Err(_) => 0,
    };
    match close(listener) {
        Result::Ok(_) => 0,
        Result::Err(_) => 0,
    };
}
