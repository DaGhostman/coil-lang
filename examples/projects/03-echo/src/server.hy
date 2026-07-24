// Server-side pure helpers for the echo demo.
// TCP accept/read/write stays in `main.hy` (IO HostInvoke from a
// dependency module currently does not run correctly).
// Cross-module `use` of sibling free-fns from a non-entry file is also
// unreliable — keep this file self-contained.

/// Echo policy: reply with the same framed bytes that arrived.
fn echo_reply([byte] frame) -> [byte] {
    return frame;
}
