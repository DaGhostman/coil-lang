//! Integration tests for `coil debug`.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn debug_batch_fib_break_bt_continue() {
    let bin = std::env::var("CARGO_BIN_EXE_coil")
        .expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let entry = manifest.join("examples/fib.hy");
    let cwd = std::env::temp_dir().join(format!("coil_debug_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).expect("temp cwd");
    let script = cwd.join("cmds.txt");
    std::fs::write(
        &script,
        "break fib\nrun\ninfo locals\nprint n\ndelete\ncontinue\nquit\n",
    )
    .expect("write script");

    let out = Command::new(&bin)
        .current_dir(&cwd)
        .args([
            "debug",
            entry.to_str().unwrap(),
            "-x",
            script.to_str().unwrap(),
            "--batch",
        ])
        .output()
        .expect("spawn coil debug");
    assert!(
        out.status.success(),
        "debug failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Breakpoint"),
        "expected breakpoint hit, stdout={stdout}"
    );
    assert!(
        stdout.contains("fib"),
        "expected fib in output, stdout={stdout}"
    );
    assert!(
        stdout.contains("n ($0)") || stdout.contains("Locals of fib"),
        "expected named local n, stdout={stdout}"
    );
    assert!(
        stdout.contains("Program exited normally"),
        "expected normal exit, stdout={stdout}"
    );
    assert!(
        !cwd.join("out.hyc").exists(),
        "debug must not write out.hyc"
    );
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn debug_batch_bad_command_exits_nonzero() {
    let bin = std::env::var("CARGO_BIN_EXE_coil")
        .expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let entry = manifest.join("examples/fib.hy");
    let cwd = std::env::temp_dir().join(format!("coil_debug_bad_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).expect("temp cwd");
    let script = cwd.join("cmds.txt");
    std::fs::write(&script, "notacommand\n").expect("write script");

    let out = Command::new(&bin)
        .current_dir(&cwd)
        .args([
            "debug",
            entry.to_str().unwrap(),
            "-x",
            script.to_str().unwrap(),
            "--batch",
        ])
        .output()
        .expect("spawn coil debug");
    assert!(
        !out.status.success(),
        "expected non-zero exit for bad command"
    );
    let _ = std::fs::remove_dir_all(&cwd);
}
