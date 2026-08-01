//! Integration tests for `coil dissect`.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn dissect_fib_fn_prints_bytecode_without_out_hyc() {
    let bin = std::env::var("CARGO_BIN_EXE_coil")
        .expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let entry = manifest.join("examples/fib.hy");
    let cwd = std::env::temp_dir().join(format!("coil_dissect_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).expect("temp cwd");

    let out = Command::new(&bin)
        .current_dir(&cwd)
        .args(["dissect", entry.to_str().unwrap(), "--fn", "fib"])
        .output()
        .expect("spawn coil dissect");
    assert!(
        out.status.success(),
        "dissect failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(";; fn fib"),
        "expected fib header, stdout={stdout}"
    );
    assert!(
        stdout.contains("CALL") || stdout.contains("TailCall"),
        "expected recursive call, stdout={stdout}"
    );
    assert!(
        !cwd.join("out.hyc").exists(),
        "dissect must not write out.hyc"
    );
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn dissect_fn_miss_exits_nonzero() {
    let bin = std::env::var("CARGO_BIN_EXE_coil")
        .expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let entry = manifest.join("examples/fib.hy");

    let out = Command::new(&bin)
        .args(["dissect", entry.to_str().unwrap(), "--fn", "nope"])
        .output()
        .expect("spawn coil dissect");
    assert!(
        !out.status.success(),
        "expected non-zero exit for unknown --fn"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no functions matching") || err.contains("E0902"),
        "stderr={err}"
    );
}
