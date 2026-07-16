use std::io::{Read, Write};
use std::process::exit;

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

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    let cli = match parse_args(&raw_args) {
        Ok(c) => c,
        Err(msg) => {
            // No pipeline yet — use a throwaway pretty sink for the flag error.
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

    let out_path = "out.c0s";
    let needs_compile = match std::fs::exists(out_path) {
        Ok(exists) => !exists,
        Err(e) => fail_and_exit(
            &mut pipeline,
            ErrorCode::IoError,
            format!("Unable to determine if `{out_path}` exists: {e}"),
        ),
    };

    if needs_compile {
        compile_to_archive(&mut pipeline, &cli.filename, out_path);
    }

    let mut f = match std::fs::File::open(out_path) {
        Ok(f) => f,
        Err(e) => fail_and_exit(
            &mut pipeline,
            ErrorCode::IoError,
            format!("Unable to open `{out_path}`: {e}"),
        ),
    };

    let mut buffer = Vec::with_capacity(1024);
    if let Err(e) = f.read_to_end(&mut buffer) {
        fail_and_exit(
            &mut pipeline,
            ErrorCode::IoError,
            format!("Unable to read `{out_path}`: {e}"),
        );
    }

    let archived = match rkyv::access::<ArchivedArchivedProgram, Error>(&buffer) {
        Ok(a) => a,
        Err(e) => fail_and_exit(
            &mut pipeline,
            ErrorCode::IoError,
            format!("Unable to decode bytecode archive: {e}"),
        ),
    };

    if archived.version != ARCHIVE_VERSION {
        fail_and_exit(
            &mut pipeline,
            ErrorCode::ArchiveVersionMismatch,
            format!(
                "Bytecode archive version {} does not match compiler version {}. Please recompile from source.",
                archived.version, ARCHIVE_VERSION
            ),
        );
    }

    let bytecode = match rkyv::deserialize::<Vec<Byte>, Error>(&archived.bytecode) {
        Ok(b) => b,
        Err(e) => fail_and_exit(
            &mut pipeline,
            ErrorCode::IoError,
            format!("Unable to deserialize bytecode: {e}"),
        ),
    };
    let constants = match rkyv::deserialize::<Vec<u64>, Error>(&archived.constants) {
        Ok(c) => c,
        Err(e) => fail_and_exit(
            &mut pipeline,
            ErrorCode::IoError,
            format!("Unable to deserialize constant pool: {e}"),
        ),
    };

    // Successful compile/load: flush any buffered sink (no-op for pretty).
    if let Err(e) = pipeline.finish_reporting() {
        eprintln!("warning: failed to flush diagnostics: {e}");
    }

    let entry = std::path::Path::new(&cli.filename);
    let mut machine = Machine::<256>::default();
    pipeline.wire_vm_ffi(&mut machine, Some(entry));
    machine.run_raw(&bytecode, &constants);
}
