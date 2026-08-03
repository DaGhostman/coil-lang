//! Integration tests for `coil fmt`.

use std::path::PathBuf;
use std::process::Command;

fn coil_bin() -> String {
    std::env::var("CARGO_BIN_EXE_coil").expect("CARGO_BIN_EXE_coil (run via `cargo test -p coil`)")
}

fn ensure_coil_fmt() {
    let coil = PathBuf::from(coil_bin());
    let helper = coil.with_file_name("coil-fmt");
    if helper.is_file() {
        return;
    }
    let status = Command::new("cargo")
        .args(["build", "-q", "-p", "coil-fmt"])
        .status()
        .expect("spawn cargo build -p coil-fmt");
    assert!(
        status.success() && helper.is_file(),
        "coil-fmt missing at {}",
        helper.display()
    );
}

fn scratch_dir(suffix: &str) -> PathBuf {
    let cwd = std::env::temp_dir().join(format!(
        "coil_fmt_{suffix}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).expect("temp cwd");
    cwd
}

#[test]
fn fmt_rewrites_messy_file_in_place() {
    ensure_coil_fmt();
    let cwd = scratch_dir("rewrite");
    let path = cwd.join("messy.hy");
    let messy = "fn  main ( ) { return  ; }\n";
    std::fs::write(&path, messy).expect("write");

    let out = Command::new(coil_bin())
        .args(["fmt", path.to_str().unwrap()])
        .output()
        .expect("spawn coil fmt");
    assert!(
        out.status.success(),
        "fmt failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let rewritten = std::fs::read_to_string(&path).expect("read");
    assert_ne!(rewritten, messy);
    assert!(
        rewritten.contains("fn main"),
        "expected formatted fn, got:\n{rewritten}"
    );

    let check = Command::new(coil_bin())
        .args(["fmt", "--check", path.to_str().unwrap()])
        .output()
        .expect("spawn coil fmt --check");
    assert!(
        check.status.success(),
        "already-formatted file should pass --check: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn fmt_check_fails_when_reformat_needed() {
    ensure_coil_fmt();
    let cwd = scratch_dir("check");
    let path = cwd.join("messy.hy");
    std::fs::write(&path, "fn  main ( ) { return  ; }\n").expect("write");
    let before = std::fs::read_to_string(&path).unwrap();

    let out = Command::new(coil_bin())
        .args(["fmt", "--check", path.to_str().unwrap()])
        .output()
        .expect("spawn coil fmt --check");
    assert!(
        !out.status.success(),
        "expected non-zero exit when reformat needed"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("would reformat") || err.contains("would be reformatted"),
        "stderr={err}"
    );
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(before, after, "--check must not rewrite");

    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn fmt_walks_directory() {
    ensure_coil_fmt();
    let cwd = scratch_dir("dir");
    let nested = cwd.join("pkg");
    std::fs::create_dir_all(&nested).unwrap();
    let a = cwd.join("a.hy");
    let b = nested.join("b.hy");
    std::fs::write(&a, "fn  main ( ) { return  ; }\n").unwrap();
    std::fs::write(&b, "fn  g ( ) { return  ; }\n").unwrap();

    let out = Command::new(coil_bin())
        .args(["fmt", cwd.to_str().unwrap()])
        .output()
        .expect("spawn coil fmt dir");
    assert!(
        out.status.success(),
        "fmt dir failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(std::fs::read_to_string(&a).unwrap().contains("fn main"));
    assert!(std::fs::read_to_string(&b).unwrap().contains("fn g"));

    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn fmt_preserves_comments_and_docs() {
    ensure_coil_fmt();
    let cwd = scratch_dir("docs");
    let path = cwd.join("doc.hy");
    let src = "\
/// Adds one.
fn  add ( int x ) -> int {
    // body note
    return x + 1;
}
";
    std::fs::write(&path, src).expect("write");

    let out = Command::new(coil_bin())
        .args(["fmt", path.to_str().unwrap()])
        .output()
        .expect("spawn coil fmt");
    assert!(
        out.status.success(),
        "fmt failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let rewritten = std::fs::read_to_string(&path).expect("read");
    assert!(
        rewritten.contains("/// Adds one."),
        "docs lost:\n{rewritten}"
    );
    assert!(
        rewritten.contains("// body note"),
        "comment lost:\n{rewritten}"
    );

    let check = Command::new(coil_bin())
        .args(["fmt", "--check", path.to_str().unwrap()])
        .output()
        .expect("spawn coil fmt --check");
    assert!(
        check.status.success(),
        "formatted docs file should pass --check: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let _ = std::fs::remove_dir_all(&cwd);
}
