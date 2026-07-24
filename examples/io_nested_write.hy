// Nested IO HostInvoke as the first of two args: `write_all(open(...), buf)`.
// Regression for emit_io_host_invoke when outer arity > 1.
use io::*;

fn main() {
    let path = "/tmp/coil_io_nested_write.bin";
    let z: byte = 0;
    let a: byte = 120;
    let b: byte = 121;
    let payload: [byte] = [a, b];
    // Nested arity>1: outer native id must precede nested open()'s HostInvoke.
    write_all(open(path, "w")?, payload)?;
    let r = open(path, "r")?;
    let got = match read_to_end(r) {
        Result::Ok(buf) => buf,
        Result::Err(_) => [z],
    };
    close(r)?;
    print "%i", len(got);
}
