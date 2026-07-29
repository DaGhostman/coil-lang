// 03-echo — single-process TCP echo (listen + connect + exchange).
//
// Modules: protocol (framing), server/client (pure helpers).
// Stream IO is in this entry file for clarity (deps may also call IO + `?`).
//
// Expected output: ok

use io::*;
use io::net::tcp::*;
use protocol::*;
use server::*;
use client::*;

async fn greeting_bytes() {
    yield 65;
    yield 66;
    return 0;
}

fn run_echo() {
    let port = client_port();
    let listener = listen("127.0.0.1", port)?;
    let client = connect("127.0.0.1", port)?;
    let server = accept_wait(listener)?;

    let h = greeting_bytes();
    let ya = resume h;
    let yb = resume h;
    let _done = resume h;
    if ya != 65 {
        close(client)?;
        close(server)?;
        close(listener)?;
        return "bad-coro";
    }
    if yb != 66 {
        close(client)?;
        close(server)?;
        close(listener)?;
        return "bad-coro";
    }

    let body = request_body();
    let frame = encode_frame(body);
    write_all(client, frame)?;

    let s0: [byte] = [0];
    let s1: [byte] = [0];
    let s2: [byte] = [0];
    read_exact(server, s0)?;
    read_exact(server, s1)?;
    read_exact(server, s2)?;

    let inbound: [byte] = [s0[0]];
    inbound[] = s1[0];
    inbound[] = s2[0];
    let reply = echo_reply(inbound);
    write_all(server, reply)?;

    let c0: [byte] = [0];
    let c1: [byte] = [0];
    let c2: [byte] = [0];
    read_exact(client, c0)?;
    read_exact(client, c1)?;
    read_exact(client, c2)?;

    close(client)?;
    close(server)?;
    close(listener)?;

    let back: [byte] = [c0[0]];
    back[] = c1[0];
    back[] = c2[0];
    if payload_eq(back, body) == 1 {
        return "ok";
    }
    return "bad";
}

fn main() {
    print "%s", match run_echo() {
        Result::Ok(s) => s,
        Result::Err(_) => "err",
    };
}
