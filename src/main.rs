use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::SystemTime;

use common::{ARCHIVE_VERSION, ArchivedArchivedProgram, ArchivedProgram, Byte};
use compiler::Pipeline;
use machine::Machine;
use reporting::{ErrorCode, ReportConfig, ReportFormat};
use rkyv::rancor::Error;

const DEFAULT_OUT: &str = "out.c0s";
const TESTS_DIR: &str = "tests";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Legacy: compile entry → out.c0s (cached) → run.
    BuildAndRun { filename: String },
    Compile { filename: String, output: String },
    Run { archive: String },
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    command: Command,
    log_json: bool,
    log_lsp: bool,
}

fn print_help() {
    eprintln!(
        "Usage:\n\
         \x20 zero-script [--log-json | --log-lsp] <file.0s>\n\
         \x20 zero-script [--log-json | --log-lsp] compile <file.0s> [-o|--output <path>]\n\
         \x20 zero-script [--log-json | --log-lsp] run <file.c0s>\n\
         \x20 zero-script [--log-json | --log-lsp] test\n\
         \n\
         Commands:\n\
         \x20 (default)  Compile <file.0s> to out.c0s (cached) and run it\n\
         \x20 compile    Compile an entry file (must define main) to a .c0s archive\n\
         \x20 run        Execute a previously compiled .c0s archive\n\
         \x20 test       Compile and run every .0s file under ./tests\n\
         \n\
         Options:\n\
         \x20 -o, --output <path>  Output archive for `compile` (default: out.c0s)\n\
         \x20 --log-json           Emit SARIF 2.1 diagnostics on stdout\n\
         \x20 --log-lsp            Emit LSP Diagnostic NDJSON on stdout\n\
         \x20 -h, --help           Show this help\n\
         \n\
         (default diagnostics) Pretty reports on stderr"
    );
}

fn parse_args(args: &[String]) -> Result<CliArgs, &'static str> {
    let mut log_json = false;
    let mut log_lsp = false;
    let mut positionals: Vec<String> = Vec::new();
    let mut output: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--log-json" => log_json = true,
            "--log-lsp" => log_lsp = true,
            "-h" | "--help" => {
                print_help();
                exit(0);
            }
            "-o" | "--output" => {
                i += 1;
                let Some(path) = args.get(i) else {
                    return Err("missing path after -o/--output");
                };
                if path.starts_with('-') {
                    return Err("missing path after -o/--output");
                }
                if output.is_some() {
                    return Err("duplicate -o/--output flag");
                }
                output = Some(path.clone());
            }
            s if s.starts_with('-') => {
                return Err("unrecognized flag (expected --log-json, --log-lsp, -o/--output, or a command/file)");
            }
            _ => positionals.push(arg.clone()),
        }
        i += 1;
    }

    let command = match positionals.as_slice() {
        [] => return Err("missing input file or command"),
        [cmd] if cmd == "test" => {
            if output.is_some() {
                return Err("-o/--output is only valid with `compile`");
            }
            Command::Test
        }
        [cmd] if cmd == "compile" => return Err("compile requires an entry file"),
        [cmd] if cmd == "run" => return Err("run requires a .c0s archive path"),
        [cmd, filename] if cmd == "compile" => {
            if filename == "compile" || filename == "run" || filename == "test" {
                return Err("compile requires an entry file");
            }
            Command::Compile {
                filename: filename.clone(),
                output: output.unwrap_or_else(|| DEFAULT_OUT.to_string()),
            }
        }
        [cmd, archive] if cmd == "run" => {
            if output.is_some() {
                return Err("-o/--output is only valid with `compile`");
            }
            Command::Run {
                archive: archive.clone(),
            }
        }
        [filename] => {
            if output.is_some() {
                return Err("-o/--output is only valid with `compile`");
            }
            Command::BuildAndRun {
                filename: filename.clone(),
            }
        }
        _ => return Err("too many arguments"),
    };

    Ok(CliArgs {
        command,
        log_json,
        log_lsp,
    })
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

fn execute_archive(
    pipeline: &Pipeline,
    bytecode: &[Byte],
    constants: &[u64],
    entry: Option<&Path>,
) {
    let mut machine = Machine::<256>::default();
    pipeline.wire_vm_ffi(&mut machine, entry);
    machine.run_raw(bytecode, constants);
}

fn cmd_build_and_run(pipeline: &mut Pipeline, filename: &str) {
    let cached = try_load_archive(DEFAULT_OUT);
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
        Ok(_) => source_newer_than_archive(filename, DEFAULT_OUT),
    };

    if recompile {
        let _ = std::fs::remove_file(DEFAULT_OUT);
        compile_to_archive(pipeline, filename, DEFAULT_OUT);
    }

    let (bytecode, constants) = if recompile {
        match try_load_archive(DEFAULT_OUT) {
            Ok(ok) => ok,
            Err(_) => fail_and_exit(
                pipeline,
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

    execute_archive(pipeline, &bytecode, &constants, Some(Path::new(filename)));
}

fn cmd_compile(pipeline: &mut Pipeline, filename: &str, output: &str) {
    compile_to_archive(pipeline, filename, output);
    if let Err(e) = pipeline.finish_reporting() {
        eprintln!("warning: failed to flush diagnostics: {e}");
    }
}

fn cmd_run(pipeline: &mut Pipeline, archive: &str) {
    let (bytecode, constants) = match try_load_archive(archive) {
        Ok(ok) => ok,
        Err(LoadErr::Missing) => fail_and_exit(
            pipeline,
            ErrorCode::IoError,
            format!("Bytecode archive `{archive}` not found"),
        ),
        Err(LoadErr::Corrupt) => fail_and_exit(
            pipeline,
            ErrorCode::IoError,
            format!("Bytecode archive `{archive}` is corrupt"),
        ),
        Err(LoadErr::Version(v)) => fail_and_exit(
            pipeline,
            ErrorCode::IoError,
            format!(
                "Bytecode archive version {v} does not match compiler version {}. Please recompile from source.",
                ARCHIVE_VERSION
            ),
        ),
    };

    if let Err(e) = pipeline.finish_reporting() {
        eprintln!("warning: failed to flush diagnostics: {e}");
    }

    // Weak base_dir: archive parent, for relative FFI dload paths.
    let entry = Path::new(archive);
    execute_archive(pipeline, &bytecode, &constants, Some(entry));
}

fn collect_test_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.is_dir() {
        return Err(format!("tests directory `{}` not found", dir.display()));
    }

    let mut files = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("unable to read `{}`: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("unable to read directory entry: {e}"))?;
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path.extension().and_then(|e| e.to_str()) == Some("0s") {
                out.push(path);
            }
        }
        Ok(())
    }
    walk(dir, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "no `.0s` test files found under `{}`",
            dir.display()
        ));
    }
    Ok(files)
}

fn cmd_test(config: ReportConfig) {
    let tests_dir = Path::new(TESTS_DIR);
    let files = match collect_test_files(tests_dir) {
        Ok(f) => f,
        Err(msg) => {
            let format = config.format;
            let mut pipeline = Pipeline::with_reporter(config, writer_for(format));
            fail_and_exit(&mut pipeline, ErrorCode::IoError, msg);
        }
    };

    let mut passed = 0usize;
    let mut failed = 0usize;

    for path in &files {
        let display = path.display().to_string();
        let format = config.format;
        let mut pipeline = Pipeline::with_reporter(config.clone(), writer_for(format));

        let compiled = pipeline.compile_src_from_file(&display);
        let _ = pipeline.finish_reporting();

        let ok = match compiled {
            Ok((bytecode, constants)) => {
                let entry = path.as_path();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_archive(&pipeline, &bytecode, &constants, Some(entry));
                }));
                result.is_ok()
            }
            Err(()) => false,
        };

        if ok {
            passed += 1;
            eprintln!("ok   {display}");
        } else {
            failed += 1;
            eprintln!("FAILED {display}");
        }
    }

    eprintln!();
    eprintln!(
        "test result: {}. {passed} passed; {failed} failed; {} total",
        if failed == 0 { "ok" } else { "FAILED" },
        passed + failed
    );

    if failed != 0 {
        exit(1);
    }
}

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    let cli = match parse_args(&raw_args) {
        Ok(c) => c,
        Err(msg) => {
            let config = ReportConfig::default();
            let mut pipeline = Pipeline::with_reporter(config, Box::new(std::io::stderr()));
            let code = if msg.contains("mutually") || msg.contains("unrecognized") || msg.contains("only valid")
                || msg.contains("duplicate")
                || msg.contains("missing path")
            {
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

    match cli.command {
        Command::Test => cmd_test(config),
        command => {
            let format = config.format;
            let mut pipeline = Pipeline::with_reporter(config, writer_for(format));
            match command {
                Command::BuildAndRun { filename } => cmd_build_and_run(&mut pipeline, &filename),
                Command::Compile { filename, output } => {
                    cmd_compile(&mut pipeline, &filename, &output)
                }
                Command::Run { archive } => cmd_run(&mut pipeline, &archive),
                Command::Test => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        std::iter::once("zero-script".to_string())
            .chain(parts.iter().map(|s| (*s).to_string()))
            .collect()
    }

    #[test]
    fn parse_legacy_build_and_run() {
        let cli = parse_args(&args(&["examples/fib.0s"])).unwrap();
        assert_eq!(
            cli.command,
            Command::BuildAndRun {
                filename: "examples/fib.0s".into()
            }
        );
        assert!(!cli.log_json);
    }

    #[test]
    fn parse_compile_default_output() {
        let cli = parse_args(&args(&["compile", "examples/fib.0s"])).unwrap();
        assert_eq!(
            cli.command,
            Command::Compile {
                filename: "examples/fib.0s".into(),
                output: DEFAULT_OUT.into(),
            }
        );
    }

    #[test]
    fn parse_compile_with_short_output() {
        let cli = parse_args(&args(&["compile", "examples/fib.0s", "-o", "fib.c0s"])).unwrap();
        assert_eq!(
            cli.command,
            Command::Compile {
                filename: "examples/fib.0s".into(),
                output: "fib.c0s".into(),
            }
        );
    }

    #[test]
    fn parse_compile_with_long_output_before_command() {
        let cli = parse_args(&args(&["--output", "x.c0s", "compile", "a.0s"])).unwrap();
        assert_eq!(
            cli.command,
            Command::Compile {
                filename: "a.0s".into(),
                output: "x.c0s".into(),
            }
        );
    }

    #[test]
    fn parse_run() {
        let cli = parse_args(&args(&["run", "out.c0s"])).unwrap();
        assert_eq!(
            cli.command,
            Command::Run {
                archive: "out.c0s".into()
            }
        );
    }

    #[test]
    fn parse_test() {
        let cli = parse_args(&args(&["test"])).unwrap();
        assert_eq!(cli.command, Command::Test);
    }

    #[test]
    fn parse_log_flags_with_subcommand() {
        let cli = parse_args(&args(&["--log-json", "compile", "a.0s"])).unwrap();
        assert!(cli.log_json);
        assert!(matches!(cli.command, Command::Compile { .. }));

        let cli = parse_args(&args(&["test", "--log-lsp"])).unwrap();
        assert!(cli.log_lsp);
        assert_eq!(cli.command, Command::Test);
    }

    #[test]
    fn parse_rejects_output_on_run_and_test() {
        assert!(parse_args(&args(&["run", "a.c0s", "-o", "x"])).is_err());
        assert!(parse_args(&args(&["test", "-o", "x"])).is_err());
        assert!(parse_args(&args(&["examples/fib.0s", "-o", "x"])).is_err());
    }

    #[test]
    fn parse_rejects_missing_compile_file() {
        assert!(parse_args(&args(&["compile"])).is_err());
        assert!(parse_args(&args(&["run"])).is_err());
        assert!(parse_args(&args(&[])).is_err());
    }

    #[test]
    fn parse_rejects_unrecognized_flag() {
        assert!(parse_args(&args(&["--bogus", "a.0s"])).is_err());
    }
}
