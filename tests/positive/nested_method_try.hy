// COI-108: nested method `?` with a different Ok payload must keep the
// inner ReturnPair (not JumpIfMatch a prematurely boxed heap enum).

class Enc {}

impl Enc {
    fn encode(int n) -> Result<Vec<byte>, string> {
        let out: Vec<byte> = Vec::new();
        out.push(n as byte);
        let m = n + 1;
        out.push(m as byte);
        return out;
    }

    fn encode_fail(int _n) -> Result<Vec<byte>, string> {
        raise "boom";
    }

    fn encode_into(int n) -> Result<int, string> {
        let bytes = self.encode(n)?;
        return len(bytes);
    }

    fn encode_first(int n) -> Result<byte, string> {
        let bytes = self.encode(n)?;
        return bytes[0];
    }

    fn encode_into_fail(int n) -> Result<int, string> {
        let bytes = self.encode_fail(n)?;
        return len(bytes);
    }
}

fn free_encode(int n) -> Result<Vec<byte>, string> {
    let out: Vec<byte> = Vec::new();
    out.push(n as byte);
    return out;
}

fn free_encode_into(int n) -> Result<int, string> {
    let bytes = free_encode(n)?;
    return len(bytes);
}

test("nested method try preserves Ok payload length") {
    let e = new Enc();
    let n = e.encode_into(10)?;
    assert(n == 2)?;
}

test("nested method try preserves Ok byte payload") {
    let e = new Enc();
    let b = e.encode_first(10)?;
    assert(b == (10 as byte))?;
}

test("nested method try propagates Err") {
    let e = new Enc();
    let msg = match e.encode_into_fail(1) {
        Result::Ok(_) => "ok",
        Result::Err(m) => m,
    };
    assert(msg == "boom")?;
}

test("nested free-fn try mismatched Result payload") {
    let n = free_encode_into(7)?;
    assert(n == 1)?;
}
