//! `coil-debug` — GDB-style REPL and DAP debug adapter.

mod dap;
mod repl;
mod session;

use std::process::exit;

pub use repl::{DebugArgs, cmd_debug};

pub fn cmd_dap() {
    if let Err(e) = dap::run_dap_server() {
        eprintln!("coil-debug: DAP error: {e}");
        exit(1);
    }
}
