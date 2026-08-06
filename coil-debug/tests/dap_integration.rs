//! Integration test for `coil-debug --dap`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn coil_debug_bin() -> PathBuf {
    for key in ["CARGO_BIN_EXE_coil_debug", "CARGO_BIN_EXE_coil-debug"] {
        if let Ok(p) = std::env::var(key) {
            let path = PathBuf::from(&p);
            if path.is_file() {
                return path;
            }
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for base in ["../target/debug/coil-debug", "../../target/debug/coil-debug"] {
        let local = manifest.join(base);
        if local.is_file() {
            return local.canonicalize().unwrap_or(local);
        }
    }
    panic!("coil-debug binary not found (run `cargo build -p coil-debug`)");
}

fn fib_entry() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("examples/fib.hy")
}

fn write_dap<W: Write>(w: &mut W, seq: i64, cmd: &str, args: serde_json::Value) {
    let body = serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": cmd,
        "arguments": args,
    });
    let bytes = serde_json::to_vec(&body).expect("json");
    write!(w, "Content-Length: {}\r\n\r\n", bytes.len()).expect("header");
    w.write_all(&bytes).expect("body");
}

#[test]
fn dap_stop_on_entry_and_continue() {
    let bin = coil_debug_bin();
    let entry = fib_entry().canonicalize().expect("fib.hy");
    let cwd = entry.parent().unwrap().parent().unwrap();

    let mut script = Vec::new();
    write_dap(
        &mut script,
        1,
        "initialize",
        serde_json::json!({ "clientID": "test", "adapterID": "coil" }),
    );
    write_dap(
        &mut script,
        2,
        "launch",
        serde_json::json!({
            "program": entry.to_string_lossy(),
            "cwd": cwd.to_string_lossy(),
            "stopOnEntry": true,
        }),
    );
    write_dap(&mut script, 3, "configurationDone", serde_json::json!({}));
    write_dap(&mut script, 4, "continue", serde_json::json!({ "threadId": 1 }));
    write_dap(&mut script, 5, "disconnect", serde_json::json!({}));

    let mut child = Command::new(&bin)
        .arg("--dap")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn coil-debug --dap");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin.write_all(&script).expect("write script");
        stdin.flush().expect("flush");
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait");

    assert!(
        output.status.success() || output.status.code() == Some(0),
        "status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"event\":\"stopped\"") && stdout.contains("\"reason\":\"entry\""),
        "expected entry stop, stdout={stdout}"
    );
    assert!(
        stdout.contains("\"event\":\"terminated\""),
        "expected terminated, stdout={stdout}"
    );
}
