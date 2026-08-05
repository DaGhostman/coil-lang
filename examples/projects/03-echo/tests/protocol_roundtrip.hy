// Pure unit test: protocol encode/decode (no sockets).
use protocol::{encode_frame, frame_len, payload_eq};

test("encode frame length") {
    let body: [byte] = [65, 66];
    let frame = encode_frame(body);
    assert(frame_len(frame) == 2, "len")?;
}

test("payload roundtrip and mismatch") {
    let body: [byte] = [65, 66];
    let frame = encode_frame(body);
    assert(payload_eq(frame, body) == 1, "roundtrip")?;
    let other: [byte] = [65, 67];
    assert(payload_eq(frame, other) == 0, "mismatch")?;
}
