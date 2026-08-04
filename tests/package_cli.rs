//! Integration tests for `coil package` (requires the real CLI binary, not the test harness).

use std::path::PathBuf;
use std::process::Command;

fn ensure_coil_embed_beside(coil: &str) {
    let coil_path = PathBuf::from(coil);
    let embed = coil_path.with_file_name("coil-embed");
    if embed.is_file() {
        return;
    }
    let status = Command::new("cargo")
        .args(["build", "-q", "-p", "coil-embed"])
        .status()
        .expect("spawn cargo build -p coil-embed");
    assert!(
        status.success() && embed.is_file(),
        "coil-embed missing at {} (build it with `cargo build -p coil-embed`)",
        embed.display()
    );
}

#[test]
fn package_fib_embedded_run_prints_55() {
    let bin = std::env::var("CARGO_BIN_EXE_coil")
        .expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)");
    ensure_coil_embed_beside(&bin);
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

    let coil_size = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
    let packaged_size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    assert!(
        packaged_size < coil_size,
        "expected packaged ({packaged_size}) < full coil ({coil_size}); is coil-embed the runner?"
    );

    let _ = std::fs::remove_file(&out);
}
