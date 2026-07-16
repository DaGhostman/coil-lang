use std::io::{Read, Write};
use std::path::Path;
use std::process::exit;
use std::time::SystemTime;

use common::{ARCHIVE_VERSION, ArchivedArchivedProgram, ArchivedProgram, Byte};
use compiler::Pipeline;
use machine::Machine;
use reporting::{ErrorCode, ReportConfig, ReportFormat};
use rkyv::rancor::Error;

struct CliArgs {
    filename: String,
    log_json: bool,
    log_lsp: bool,
}

fn parse_args(args: &[String]) -> Result<CliArgs, &'static str> {
    let mut log_json = false;
    let mut log_lsp = false;
    let mut filename: Option<String> = None;

    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--log-json" => log_json = true,
            "--log-lsp" => log_lsp = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: zero-script [--log-json | --log-lsp] <file.0s>\n\
                     \n\
                     --log-json  Emit SARIF 2.1 diagnostics on stdout\n\
                     --log-lsp   Emit LSP Diagnostic NDJSON on stdout\n\
                     (default)   Pretty diagnostics on stderr"
                );
                exit(0);
            }
            s if s.starts_with('-') => {
                return Err("unrecognized flag (expected --log-json, --log-lsp, or a source file)");
            }
            _ => {
                if filename.is_some() {
                    return Err("expected a single input file");
                }
                filename = Some(arg.clone());
            }
        }
    }

    match filename {
        Some(filename) => Ok(CliArgs {
            filename,
            log_json,
            log_lsp,
        }),
        None => Err("missing input file"),
    }
}

fn writer_for(format: ReportFormat) -> Box<dyn Write + Send> {
    match format {
        ReportFormat::Pretty => Box::new(std::io::stderr()),
        ReportFormat::Sarif | ReportFormat::Lsp => Box::new(std::io::stdout()),
    }
}

fn fail_and_exit(pipeline: &mut Pipeline, code: ErrorCode, message: impl Into<String>) -> ! {
    pipeline.emit_spanless_error(code, message);
    let _ = pipeline.finish_reporting();
    exit(1);
}

fn compile_to_archive(pipeline: &mut Pipeline, filename: &str, output: &str) {
    // Multi-file entry: discovers `use` / `mod` via zero.toml.
    let (bytecode, constants) = match pipeline.compile_src_from_file(filename) {
        Ok(ok) => ok,
        Err(()) => {
            let _ = pipeline.finish_reporting();
            exit(1);
        }
    };

    let program = ArchivedProgram {
        version: ARCHIVE_VERSION,
        constants,
        bytecode,
    };

    let bytes = match rkyv::to_bytes::<Error>(&program) {
        Ok(b) => b,
        Err(e) => fail_and_exit(
            pipeline,
            ErrorCode::IoError,
            format!("Unable to serialize bytecode archive: {e}"),
        ),
    };

    if let Err(e) = std::fs::write(output, bytes.as_slice()) {
        fail_and_exit(
            pipeline,
            ErrorCode::IoError,
            format!("Unable to write compiled output to `{output}`: {e}"),
        );
    }
}

fn archive_mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn source_newer_than_archive(filename: &str, archive: &str) -> bool {
    match (archive_mtime(archive), archive_mtime(filename)) {
        (Some(arch), Some(src)) => src > arch,
        _ => false,
    }
}

fn try_load_archive(path: &str) -> Result<(Vec<Byte>, Vec<u64>), LoadErr> {
    let mut f = std::fs::File::open(path).map_err(|_| LoadErr::Missing)?;
    let mut buffer = Vec::with_capacity(1024);
    f.read_to_end(&mut buffer).map_err(|_| LoadErr::Corrupt)?;
    let archived =
        rkyv::access::<ArchivedArchivedProgram, Error>(&buffer).map_err(|_| LoadErr::Corrupt)?;
    let version = u32::from(archived.version);
    if version != ARCHIVE_VERSION {
        return Err(LoadErr::Version(version));
    }
    let bytecode = rkyv::deserialize::<Vec<Byte>, Error>(&archived.bytecode)
        .map_err(|_| LoadErr::Corrupt)?;
    let constants = rkyv::deserialize::<Vec<u64>, Error>(&archived.constants)
        .map_err(|_| LoadErr::Corrupt)?;
    Ok((bytecode, constants))
}

#[derive(Debug)]
enum LoadErr {
    Missing,
    Corrupt,
    Version(u32),
}

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    let cli = match parse_args(&raw_args) {
        Ok(c) => c,
        Err(msg) => {
            let config = ReportConfig::default();
            let mut pipeline = Pipeline::with_reporter(config, Box::new(std::io::stderr()));
            let code = if msg.contains("mutually") || msg.contains("unrecognized") {
                ErrorCode::InvalidCliFlags
            } else {
                ErrorCode::MissingInputFile
            };
            fail_and_exit(&mut pipeline, code, msg);
        }
    };

    let config = match ReportConfig::from_cli_flags(cli.log_json, cli.log_lsp) {
        Ok(c) => c,
        Err(msg) => {
            let mut pipeline =
                Pipeline::with_reporter(ReportConfig::default(), Box::new(std::io::stderr()));
            fail_and_exit(&mut pipeline, ErrorCode::InvalidCliFlags, msg);
        }
    };

    let format = config.format;
    let mut pipeline = Pipeline::with_reporter(config, writer_for(format));

    const OUT: &str = "out.c0s";

    let cached = try_load_archive(OUT);
    let recompile = match &cached {
        Err(LoadErr::Missing) => true,
        Err(LoadErr::Corrupt) => true,
        Err(LoadErr::Version(v)) => {
            // Stale archive: recompile rather than hard-fail (main #8 behavior).
            eprintln!(
                "Bytecode archive version {} does not match compiler version {}. Recompiling.",
                v, ARCHIVE_VERSION
            );
            true
        }
        Ok(_) => source_newer_than_archive(&cli.filename, OUT),
    };

    if recompile {
        let _ = std::fs::remove_file(OUT);
        compile_to_archive(&mut pipeline, &cli.filename, OUT);
    }

    let (bytecode, constants) = if recompile {
        match try_load_archive(OUT) {
            Ok(ok) => ok,
            Err(_) => fail_and_exit(
                &mut pipeline,
                ErrorCode::IoError,
                "Unable to load freshly compiled archive",
            ),
        }
    } else {
        cached.expect("archive checked above")
    };

    if let Err(e) = pipeline.finish_reporting() {
        eprintln!("warning: failed to flush diagnostics: {e}");
    }

    let entry = Path::new(&cli.filename);
    let mut machine = Machine::<256>::default();
    pipeline.wire_vm_ffi(&mut machine, Some(entry));
    machine.run_raw(&bytecode, &constants);
}
