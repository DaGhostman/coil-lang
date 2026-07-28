// Server-side pure helpers for the echo demo.
// TCP accept/read/write stays in `main.hy` for layout clarity.

/// Echo policy: reply with the same framed bytes that arrived.
fn echo_reply([byte] frame) -> [byte] {
    return frame;
}
