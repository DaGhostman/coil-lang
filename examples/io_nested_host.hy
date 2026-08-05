// Nested IO HostInvoke: `read_to_end(open(...))` must pass the stream, not
// the native id, into MakeTuple (regression for emit_io_host_invoke arg order).
use io::*;
use io::sync::{read_to_end, write_all};

use string::*;

fn main() {
    let path = "/tmp/coil_io_nested_host.bin";
    let z: byte = 0;
    let a: byte = 97;
    let b: byte = 98;
    let c: byte = 99;
    let payload: [byte] = [a, b, c];
    let w = open(path, "w")?;
    write_all(w, payload)?;
    close(w)?;
    let got = match read_to_end(open(path, "r")?) {
        Result::Ok(buf) => buf,
        Result::Err(_) => [z],
    };
    write_all(stdout(), to_bytes(format("%i", len(got))));
}
