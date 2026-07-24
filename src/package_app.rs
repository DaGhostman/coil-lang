//! `coil package` and embedded-archive startup.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

use common::{
    append_package_payload, bytecode_uses_ffi, embedded_archive_slice, ffi_library_names_from_bytecode,
    is_packaged_executable, read_package_trailer, ArchivedArchivedProgram, ArchivedProgram,
    Byte, ProgramDebug, ARCHIVE_VERSION, PACKAGE_FLAG_USES_FFI,
};
use compiler::Pipeline;
use machine::{check_native_libraries, packaged_app_ffi_startup_check};
use reporting::ErrorCode;
use rkyv::rancor::Error;

use crate::{execute_archive, fail_and_exit, LoadErr};

/// Deserialize an `ArchivedProgram` blob (from `.hyc` or an embedded slice).
pub fn load_archive_bytes(
    buffer: &[u8],
) -> Result<(Vec<Byte>, Vec<u64>, u32, ProgramDebug), LoadErr> {
    let archived =
        rkyv::access::<ArchivedArchivedProgram, Error>(buffer).map_err(|_| LoadErr::Corrupt)?;
    let version = u32::from(archived.version);
    if version != ARCHIVE_VERSION {
        return Err(LoadErr::Version(version));
    }
    let bytecode = rkyv::deserialize::<Vec<Byte>, Error>(&archived.bytecode)
        .map_err(|_| LoadErr::Corrupt)?;
    let constants = rkyv::deserialize::<Vec<u64>, Error>(&archived.constants)
        .map_err(|_| LoadErr::Corrupt)?;
    let static_slot_count = u32::from(archived.static_slot_count);
    let source_files = rkyv::deserialize::<Vec<String>, Error>(&archived.source_files)
        .map_err(|_| LoadErr::Corrupt)?;
    let debug_locs = rkyv::deserialize::<Vec<common::DebugLoc>, Error>(&archived.debug_locs)
        .map_err(|_| LoadErr::Corrupt)?;
    Ok((
        bytecode,
        constants,
        static_slot_count,
        ProgramDebug {
            source_files,
            debug_locs,
        },
    ))
}

fn compile_program_archive_bytes(
    pipeline: &mut Pipeline,
    filename: &str,
    strip_debug: bool,
) -> Result<Vec<u8>, ()> {
    let (bytecode, constants) = pipeline.compile_src_from_file(filename)?;
    let debug = pipeline.program_debug();
    let (source_files, debug_locs) = if strip_debug {
        (Vec::new(), Vec::new())
    } else {
        (debug.source_files, debug.debug_locs)
    };
    let program = ArchivedProgram {
        version: ARCHIVE_VERSION,
        static_slot_count: pipeline.static_slot_count(),
        constants,
        bytecode,
        source_files,
        debug_locs,
    };
    rkyv::to_bytes::<Error>(&program)
        .map(|b| b.as_slice().to_vec())
        .map_err(|_| ())
}
fn resolve_runner_path(runner: Option<&Path>) -> Result<PathBuf, String> {
    match runner {
        Some(p) => Ok(p.to_path_buf()),
        None => std::env::current_exe().map_err(|e| format!("cannot resolve current executable: {e}")),
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o111);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// If this process is a packaged binary, run the embedded program and return `Some(panicked)`.
pub fn try_run_embedded() -> Option<bool> {
    let exe = std::env::current_exe().ok()?;
    let data = fs::read(&exe).ok()?;
    let trailer = read_package_trailer(&data)?;
    let archive = embedded_archive_slice(&data, trailer)?;

    if trailer.archive_version != ARCHIVE_VERSION {
        eprintln!(
            "embedded bytecode version {} does not match this runner ({}); rebuild with `coil package`",
            trailer.archive_version, ARCHIVE_VERSION
        );
        exit(1);
    }

    let (bytecode, constants, static_slots, debug) = match load_archive_bytes(archive) {
        Ok(ok) => ok,
        Err(LoadErr::Version(v)) => {
            eprintln!(
                "embedded archive version {v} does not match compiler version {ARCHIVE_VERSION}"
            );
            exit(1);
        }
        Err(_) => {
            eprintln!("embedded bytecode archive is corrupt");
            exit(1);
        }
    };

    if let Err(msg) = packaged_app_ffi_startup_check(trailer.uses_ffi()) {
        eprintln!("error: {msg}");
        exit(1);
    }

    let pipeline = Pipeline::new();
    let entry = exe.as_path();
    let panicked = execute_archive(
        &pipeline,
        &bytecode,
        &constants,
        static_slots,
        debug,
        Some(entry),
    );
    Some(panicked)
}

pub fn cmd_package(
    pipeline: &mut Pipeline,
    filename: &str,
    output: &str,
    runner: Option<&Path>,
    check_native: bool,
    strip_debug: bool,
) {
    let archive_bytes = match compile_program_archive_bytes(pipeline, filename, strip_debug) {
        Ok(b) => b,
        Err(()) => {
            let _ = pipeline.finish_reporting();
            exit(1);
        }
    };

    let program = rkyv::access::<ArchivedArchivedProgram, Error>(&archive_bytes)
        .expect("freshly serialized archive");
    let bytecode: Vec<Byte> =
        rkyv::deserialize::<Vec<Byte>, Error>(&program.bytecode).expect("bytecode");
    let uses_ffi = bytecode_uses_ffi(&bytecode);
    let mut flags = 0u32;
    if uses_ffi {
        flags |= PACKAGE_FLAG_USES_FFI;
    }

    if uses_ffi {
        eprintln!(
            "note: this program uses FFI. Target machines need the shared libraries it loads \
             (libffi is linked into this runner; user `.so` / `.dll` files are not bundled)."
        );
    }

    let base_dir = Path::new(filename).parent().filter(|p| !p.as_os_str().is_empty());
    if check_native && uses_ffi {
        let libs = ffi_library_names_from_bytecode(&bytecode);
        if let Err(msg) = check_native_libraries(&libs, base_dir) {
            fail_and_exit(pipeline, ErrorCode::IoError, msg);
        }
        if libs.is_empty() {
            // `extern "c"` and compile-time FFI may not leave STRING literals before FfiLoad.
            if let Err(msg) = check_native_libraries(&["c".to_string()], base_dir) {
                fail_and_exit(pipeline, ErrorCode::IoError, msg);
            }
        }
    }

    let runner_path = match resolve_runner_path(runner) {
        Ok(p) => p,
        Err(msg) => fail_and_exit(pipeline, ErrorCode::IoError, msg),
    };
    let runner_bytes = match fs::read(&runner_path) {
        Ok(b) => b,
        Err(e) => fail_and_exit(
            pipeline,
            ErrorCode::IoError,
            format!("cannot read runner `{}`: {e}", runner_path.display()),
        ),
    };

    if is_packaged_executable(&runner_bytes) {
        fail_and_exit(
            pipeline,
            ErrorCode::IoError,
            format!(
                "runner `{}` is already a packaged executable; use an unpackaged `coil` binary as the template",
                runner_path.display()
            ),
        );
    }

    let packaged = append_package_payload(
        &runner_bytes,
        &archive_bytes,
        flags,
        ARCHIVE_VERSION,
    );

    if let Err(e) = fs::write(output, &packaged) {
        fail_and_exit(
            pipeline,
            ErrorCode::IoError,
            format!("cannot write packaged output `{}`: {e}", output),
        );
    }
    make_executable(Path::new(output));

    if let Err(e) = pipeline.finish_reporting() {
        pipeline.emit_spanless_warning(
            ErrorCode::IoError,
            format!("failed to flush diagnostics: {e}"),
        );
        let _ = pipeline.finish_reporting();
    }

    eprintln!(
        "packaged `{}` for {}-{} ({} bytes)",
        output,
        std::env::consts::OS,
        std::env::consts::ARCH,
        packaged.len()
    );
}

/// Run a freshly packaged binary (integration tests).
#[cfg(test)]
pub fn run_packaged_output(path: &Path) -> Result<String, String> {
    use std::process::Command as StdCommand;
    let out = StdCommand::new(path)
        .output()
        .map_err(|e| format!("spawn {}: {e}", path.display()))?;
    if !out.status.success() {
        return Err(format!(
            "exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_archive_bytes_rejects_version() {
        let program = ArchivedProgram {
            version: ARCHIVE_VERSION.wrapping_sub(1),
            static_slot_count: 0,
            constants: vec![],
            bytecode: vec![Byte::new(common::Instruction::HALT)],
            source_files: vec![],
            debug_locs: vec![],
        };
        let bytes = rkyv::to_bytes::<Error>(&program).unwrap();
        assert!(matches!(
            load_archive_bytes(bytes.as_slice()),
            Err(LoadErr::Version(_))
        ));
    }
}
