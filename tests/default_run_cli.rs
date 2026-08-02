//! Integration tests for default `coil <file.hy>` (build-and-run).

use std::path::PathBuf;
use std::process::Command;

fn coil_bin() -> String {
    std::env::var("CARGO_BIN_EXE_coil").expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)")
}

fn fib_entry() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/fib.hy")
}

#[test]
fn default_run_fib_and_no_out_hyc() {
    let bin = coil_bin();
    let entry = fib_entry();
    let cwd = std::env::temp_dir().join(format!(
        "coil_default_run_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).expect("temp cwd");

    let out = Command::new(&bin)
        .current_dir(&cwd)
        .arg(entry.to_str().unwrap())
        .output()
        .expect("spawn coil");

    assert!(
        out.status.success(),
        "default run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !cwd.join("out.hyc").exists(),
        "default run must not write out.hyc"
    );
    let _ = std::fs::remove_dir_all(&cwd);
}
