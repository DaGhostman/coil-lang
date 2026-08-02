//! Integration tests for `coil package` (requires the real CLI binary, not the test harness).

use std::path::PathBuf;
use std::process::Command;

#[test]
fn package_fib_embedded_run_prints_55() {
    let bin = std::env::var("CARGO_BIN_EXE_coil")
        .expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let entry = manifest.join("examples/fib.hy");
    let out = std::env::temp_dir().join(format!("coil_fib_pack_{}", std::process::id()));
    let _ = std::fs::remove_file(&out);

    let status = Command::new(&bin)
        .args([
            "package",
            entry.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn coil package");
    assert!(status.success(), "package failed");

    let run = Command::new(&out).output().expect("run packaged binary");
    assert!(
        run.status.success(),
        "packaged app failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("55"),
        "expected fib(10)=55, stdout={stdout}"
    );
    let _ = std::fs::remove_file(&out);
}
