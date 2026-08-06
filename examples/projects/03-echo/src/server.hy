// Server-side pure helpers for the echo demo.
// TCP accept/read/write stays in `main.hy` for layout clarity.

/// Echo policy: reply with the same framed bytes that arrived.
fn echo_reply(Vec<byte> frame) -> Vec<byte> {
    return frame;
}
