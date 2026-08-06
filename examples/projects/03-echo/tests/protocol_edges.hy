// Edge cases for length-prefixed frames: empty body, truncate at 5.
use protocol::{encode_frame, frame_len, payload_eq};

test("empty frame") {
    let empty: [byte] = [];
    let eframe = encode_frame(empty);
    assert(frame_len(eframe) == 0, "empty len")?;
    assert(payload_eq(eframe, empty) == 1, "empty roundtrip")?;
}

test("truncate payload at five bytes") {
    let long: [byte] = [1, 2, 3, 4, 5, 6, 7];
    let capped = encode_frame(long);
    assert(frame_len(capped) == 5, "cap at 5")?;
    let expect: [byte] = [1, 2, 3, 4, 5];
    assert(payload_eq(capped, expect) == 1, "truncated payload")?;
}

test("one byte frame") {
    let short: [byte] = [9];
    let one = encode_frame(short);
    assert(frame_len(one) == 1, "len 1")?;
    assert(payload_eq(one, short) == 1, "one byte")?;
}
