//! Integration tests for `coil dissect`.

use std::path::PathBuf;
use std::process::Command;

fn coil_bin() -> String {
    std::env::var("CARGO_BIN_EXE_coil").expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)")
}

fn fib_entry() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/fib.hy")
}

#[test]
fn dissect_fib_fn_prints_bytecode_without_out_hyc() {
    let bin = coil_bin();
    let entry = fib_entry();
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
    let bin = coil_bin();
    let entry = fib_entry();

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

#[test]
fn dissect_fib_il_and_ast_sections() {
    let bin = coil_bin();
    let entry = fib_entry();
    let cwd = std::env::temp_dir().join(format!("coil_dissect_il_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).expect("temp cwd");

    let out = Command::new(&bin)
        .current_dir(&cwd)
        .args([
            "dissect",
            entry.to_str().unwrap(),
            "--fn",
            "fib",
            "--il",
            "--ast",
        ])
        .output()
        .expect("spawn coil dissect --il --ast");
    assert!(
        out.status.success(),
        "dissect --il --ast failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("=== bytecode ==="),
        "missing bytecode section, stdout={stdout}"
    );
    assert!(
        stdout.contains("=== il ==="),
        "missing il section, stdout={stdout}"
    );
    assert!(
        stdout.contains("=== ast ==="),
        "missing ast section, stdout={stdout}"
    );
    assert!(
        stdout.to_ascii_lowercase().contains("fib"),
        "expected fib in IL/AST output, stdout={stdout}"
    );
    assert!(
        !cwd.join("out.hyc").exists(),
        "dissect --il/--ast must not write out.hyc"
    );
    let _ = std::fs::remove_dir_all(&cwd);
}
