use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::SystemTime;

use common::{ARCHIVE_VERSION, ArchivedProgram, Byte, ProgramDebug};
use compiler::Pipeline;
use machine::Machine;
use reporting::{ErrorCode, ReportConfig, ReportFormat};
use rkyv::rancor::Error;

const DEFAULT_OUT: &str = "out.c0s";
const TESTS_DIR: &str = "tests";

mod package_app;

use package_app::{cmd_package, load_archive_bytes, try_run_embedded};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Legacy: compile entry → out.c0s (cached) → run.
    BuildAndRun { filename: String },
    Compile { filename: String, output: String },
    Run { archive: String },
    Test {
        path: Option<String>,
        fail_fast: bool,
    },
    Package {
        filename: String,
        output: String,
        runner: Option<PathBuf>,
        check_native: bool,
        strip_debug: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    command: Command,
    log_json: bool,
    log_lsp: bool,
    include_tests: bool,
}

fn print_help() {
    eprintln!(
        "Usage:\n\
         \x20 zero-script [--log-json | --log-lsp] <file.0s>\n\
         \x20 zero-script [--log-json | --log-lsp] compile <file.0s> [-o|--output <path>]\n\
         \x20 zero-script [--log-json | --log-lsp] run <file.c0s>\n\
         \x20 zero-script [--log-json | --log-lsp] package <file.0s> [-o|--output <path>]\n\
         \x20 zero-script [--log-json | --log-lsp] test [path] [--fail-fast]\n\
         \n\
         Commands:\n\
         \x20 (default)  Compile <file.0s> to out.c0s (cached) and run it\n\
         \x20 compile    Compile an entry file (must define main) to a .c0s archive\n\
         \x20 run        Execute a previously compiled .c0s archive\n\
         \x20 package    Build a single-host executable (runner + embedded .c0s)\n\
         \x20 test       Compile and run every .0s file under [path] (default: ./tests)\n\
         \x20             Files under a `compile_fail/` directory must be rejected with diagnostics\n\
         \n\
         Options:\n\
         \x20 -o, --output <path>  Output archive for `compile` or packaged binary for `package`\n\
         \x20 --runner <path>       Runner template for `package` (default: current executable)\n\
         \x20 --check-native        With `package`, fail if required shared libraries are missing\n\
         \x20 --strip-debug         With `package`, omit debug line table from embedded archive\n\
         \x20 --include-tests      Compile harness tests into the archive (default: omit)\n\
         \x20 --fail-fast          With `test`, stop after the first failed case\n\
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
    let mut fail_fast = false;
    let mut include_tests = false;
    let mut check_native = false;
    let mut strip_debug = false;
    let mut runner: Option<PathBuf> = None;
    let mut positionals: Vec<String> = Vec::new();
    let mut output: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--log-json" => log_json = true,
            "--log-lsp" => log_lsp = true,
            "--fail-fast" => fail_fast = true,
            "--include-tests" => include_tests = true,
            "--check-native" => check_native = true,
            "--strip-debug" => strip_debug = true,
            "--runner" => {
                i += 1;
                let Some(path) = args.get(i) else {
                    return Err("missing path after --runner");
                };
                if path.starts_with('-') {
                    return Err("missing path after --runner");
                }
                runner = Some(PathBuf::from(path));
            }
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
                return Err("unrecognized flag (expected --log-json, --log-lsp, --fail-fast, --include-tests, --check-native, --strip-debug, --runner, -o/--output, or a command/file)");
            }
            _ => positionals.push(arg.clone()),
        }
        i += 1;
    }

    let command = match positionals.as_slice() {
        [] => return Err("missing input file or command"),
        [cmd] if cmd == "test" => {
            if output.is_some() {
                return Err("-o/--output is only valid with `compile` or `package`");
            }
            if include_tests {
                return Err("--include-tests is only valid with `compile` or the default run mode");
            }
            if check_native || strip_debug || runner.is_some() {
                return Err("--check-native, --strip-debug, and --runner are only valid with `package`");
            }
            Command::Test {
                path: None,
                fail_fast,
            }
        }
        [cmd, path] if cmd == "test" => {
            if output.is_some() {
                return Err("-o/--output is only valid with `compile` or `package`");
            }
            if include_tests {
                return Err("--include-tests is only valid with `compile` or the default run mode");
            }
            if check_native || strip_debug || runner.is_some() {
                return Err("--check-native, --strip-debug, and --runner are only valid with `package`");
            }
            if path == "compile" || path == "run" || path == "test" || path == "package" {
                return Err("test path must be a directory");
            }
            Command::Test {
                path: Some(path.clone()),
                fail_fast,
            }
        }
        [cmd] if cmd == "compile" => return Err("compile requires an entry file"),
        [cmd] if cmd == "run" => return Err("run requires a .c0s archive path"),
        [cmd] if cmd == "package" => return Err("package requires an entry file"),
        [cmd, filename] if cmd == "package" => {
            if filename == "package" || filename == "compile" || filename == "run" || filename == "test" {
                return Err("package requires an entry file");
            }
            if fail_fast {
                return Err("--fail-fast is only valid with `test`");
            }
            if include_tests {
                return Err("--include-tests is not valid with `package`");
            }
            let out = output.unwrap_or_else(|| {
                Path::new(filename)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("a.out")
                    .to_string()
            });
            Command::Package {
                filename: filename.clone(),
                output: out,
                runner,
                check_native,
                strip_debug,
            }
        }
        [cmd, filename] if cmd == "compile" => {
            if filename == "compile" || filename == "run" || filename == "test" || filename == "package" {
                return Err("compile requires an entry file");
            }
            if fail_fast {
                return Err("--fail-fast is only valid with `test`");
            }
            if check_native || strip_debug || runner.is_some() {
                return Err("--check-native, --strip-debug, and --runner are only valid with `package`");
            }
            Command::Compile {
                filename: filename.clone(),
                output: output.unwrap_or_else(|| DEFAULT_OUT.to_string()),
            }
        }
        [cmd, archive] if cmd == "run" => {
            if output.is_some() {
                return Err("-o/--output is only valid with `compile` or `package`");
            }
            if fail_fast {
                return Err("--fail-fast is only valid with `test`");
            }
            if check_native || strip_debug || runner.is_some() {
                return Err("--check-native, --strip-debug, and --runner are only valid with `package`");
            }
            Command::Run {
                archive: archive.clone(),
            }
        }
        [filename] => {
            if output.is_some() {
                return Err("-o/--output is only valid with `compile` or `package`");
            }
            if fail_fast {
                return Err("--fail-fast is only valid with `test`");
            }
            if check_native || strip_debug || runner.is_some() {
                return Err("--check-native, --strip-debug, and --runner are only valid with `package`");
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
        include_tests,
    })
}

fn writer_for(format: ReportFormat) -> Box<dyn Write + Send> {
    match format {
        ReportFormat::Pretty => Box::new(std::io::stderr()),
        ReportFormat::Sarif | ReportFormat::Lsp => Box::new(std::io::stdout()),
    }
}

pub(crate) fn fail_and_exit(pipeline: &mut Pipeline, code: ErrorCode, message: impl Into<String>) -> ! {
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

    let debug = pipeline.program_debug();

    let program = ArchivedProgram {
        version: ARCHIVE_VERSION,
        static_slot_count: pipeline.static_slot_count(),
        constants,
        bytecode,
        source_files: debug.source_files,
        debug_locs: debug.debug_locs,
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

fn try_load_archive(path: &str) -> Result<(Vec<Byte>, Vec<u64>, u32, ProgramDebug), LoadErr> {
    let mut f = std::fs::File::open(path).map_err(|_| LoadErr::Missing)?;
    let mut buffer = Vec::with_capacity(1024);
    f.read_to_end(&mut buffer).map_err(|_| LoadErr::Corrupt)?;
    load_archive_bytes(&buffer)
}

#[derive(Debug)]
pub(crate) enum LoadErr {
    Missing,
    Corrupt,
    Version(u32),
}

/// Run archived bytecode. Returns `true` when a language-level `panic` aborted.
pub(crate) fn execute_archive(
    pipeline: &Pipeline,
    bytecode: &[Byte],
    constants: &[u64],
    static_slots: u32,
    debug: ProgramDebug,
    entry: Option<&Path>,
) -> bool {
    let mut machine = Machine::<256>::default();
    pipeline.wire_vm_ffi(&mut machine, entry);
    pipeline.wire_host_natives(&mut machine);
    machine.set_program_debug(debug);
    machine.run_raw(bytecode, constants, static_slots);
    machine.panicked()
}

fn cmd_build_and_run(pipeline: &mut Pipeline, filename: &str) {
    let cached = try_load_archive(DEFAULT_OUT);
    let recompile = match &cached {
        Err(LoadErr::Missing) => true,
        Err(LoadErr::Corrupt) => true,
        Err(LoadErr::Version(v)) => {
            pipeline.emit_spanless_error(
                ErrorCode::ArchiveVersionMismatch,
                format!(
                    "Bytecode archive version {v} does not match compiler version {ARCHIVE_VERSION}. Recompiling."
                ),
            );
            true
        }
        Ok(_) => source_newer_than_archive(filename, DEFAULT_OUT),
    };

    if recompile {
        let _ = std::fs::remove_file(DEFAULT_OUT);
        compile_to_archive(pipeline, filename, DEFAULT_OUT);
    }

    let (bytecode, constants, static_slots, debug) = if recompile {
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
        pipeline.emit_spanless_warning(
            ErrorCode::IoError,
            format!("failed to flush diagnostics: {e}"),
        );
        let _ = pipeline.finish_reporting();
    }

    if execute_archive(
        pipeline,
        &bytecode,
        &constants,
        static_slots,
        debug,
        Some(Path::new(filename)),
    ) {
        exit(1);
    }
}

fn cmd_compile(pipeline: &mut Pipeline, filename: &str, output: &str) {
    compile_to_archive(pipeline, filename, output);
    if let Err(e) = pipeline.finish_reporting() {
        pipeline.emit_spanless_warning(
            ErrorCode::IoError,
            format!("failed to flush diagnostics: {e}"),
        );
        let _ = pipeline.finish_reporting();
    }
}

fn cmd_run(pipeline: &mut Pipeline, archive: &str) {
    let (bytecode, constants, static_slots, debug) = match try_load_archive(archive) {
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
        pipeline.emit_spanless_warning(
            ErrorCode::IoError,
            format!("failed to flush diagnostics: {e}"),
        );
        let _ = pipeline.finish_reporting();
    }

    // Weak base_dir: archive parent, for relative FFI dload paths.
    let entry = Path::new(archive);
    if execute_archive(
        pipeline,
        &bytecode,
        &constants,
        static_slots,
        debug,
        Some(entry),
    ) {
        exit(1);
    }
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

/// Negative syntax / type tests live under any path segment named `compile_fail`.
/// Those files must fail to compile; a successful compile is a harness failure.
fn is_compile_fail(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str() == "compile_fail")
}

/// Classify a `catch_unwind` compile result for a `compile_fail/` file.
/// Only a clean diagnostic rejection (`Ok(Err(()))`) is harness success.
/// Panic does not count (release builds use `panic = "abort"`).
fn compile_fail_rejected<T>(compiled: &std::thread::Result<Result<T, ()>>) -> bool {
    matches!(compiled, Ok(Err(())))
}

fn run_test_case(
    pipeline: &Pipeline,
    bytecode: &[Byte],
    constants: &[u64],
    entry: Option<&Path>,
    name: &str,
    offset: u32,
) -> bool {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut machine = Machine::<256>::default();
        pipeline.wire_vm_ffi(&mut machine, entry);
        pipeline.wire_host_natives(&mut machine);
        machine.set_program_debug(pipeline.program_debug());
        machine.load_program(bytecode, constants);
        let ret = machine.call_function(offset, &[]);
        !machine.panicked() && machine.result_is_ok(ret)
    }));
    match result {
        Ok(ok) => {
            if !ok {
                eprintln!("> Test \"{name}\" failed");
            }
            ok
        }
        Err(_) => {
            eprintln!("> Test \"{name}\" failed");
            false
        }
    }
}

/// Run the test harness over `root` and return `(passed, failed)` without exiting.
/// Extracted from `cmd_test` so unit tests can assert compile_fail inversion and
/// fail-fast behavior without terminating the process.
fn run_test_suite(config: ReportConfig, root: &Path, fail_fast: bool) -> Result<(usize, usize), String> {
    let files = collect_test_files(root)?;

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut stop = false;

    for path in &files {
        if stop {
            break;
        }
        let display = path.display().to_string();
        let expect_compile_fail = is_compile_fail(path);
        let format = config.format;
        // Expected compile rejection: suppress ariadne noise so the harness
        // summary stays readable when many compile_fail files exist.
        let mut pipeline = if expect_compile_fail {
            Pipeline::with_reporter(config.clone(), Box::new(std::io::sink()))
        } else {
            Pipeline::with_reporter(config.clone(), writer_for(format))
        };
        pipeline.set_include_tests(true);

        // catch_unwind isolates a compiler ICE from aborting the whole
        // harness under panic=unwind. Release builds use panic=abort, so
        // compile_fail fixtures must reject via Ok(Err(())), not panic.
        let compiled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pipeline.compile_src_from_file(&display)
        }));
        let cases: Vec<(String, u32)> = pipeline.test_cases().to_vec();
        let _ = pipeline.finish_reporting();

        let file_ok = if expect_compile_fail {
            // Only a clean diagnostic rejection counts. A panic is a
            // harness failure (and aborts under release panic=abort).
            if compile_fail_rejected(&compiled) {
                passed += 1;
                true
            } else {
                failed += 1;
                match &compiled {
                    Ok(Ok(_)) => {
                        eprintln!("> Test \"{display}\" failed (expected compile failure)");
                    }
                    Err(_) => {
                        eprintln!("> Test \"{display}\" failed (compiler panicked)");
                    }
                    Ok(Err(())) => unreachable!("compile_fail_rejected is true for Ok(Err)"),
                }
                if fail_fast {
                    stop = true;
                }
                false
            }
        } else {
            match compiled {
                Err(_) => {
                    failed += 1;
                    eprintln!("> Test \"{display}\" failed (compiler panicked)");
                    if fail_fast {
                        stop = true;
                    }
                    false
                }
                Ok(Err(())) => {
                    failed += 1;
                    eprintln!("> Test \"{display}\" failed");
                    if fail_fast {
                        stop = true;
                    }
                    false
                }
                Ok(Ok((bytecode, constants))) => {
                    let static_slots = pipeline.static_slot_count();
                    let entry = path.as_path();
                    if cases.is_empty() {
                        // Legacy: whole-file `main` is one opaque case.
                        let debug = pipeline.program_debug();
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            execute_archive(
                                &pipeline,
                                &bytecode,
                                &constants,
                                static_slots,
                                debug,
                                Some(entry),
                            )
                        }));
                        let ok = match result {
                            Ok(panicked) => !panicked,
                            Err(_) => false,
                        };
                        if ok {
                            passed += 1;
                        } else {
                            failed += 1;
                            eprintln!("> Test \"{display}\" failed");
                            if fail_fast {
                                stop = true;
                            }
                        }
                        ok
                    } else {
                        let mut any_fail = false;
                        for (name, offset) in &cases {
                            let ok = run_test_case(
                                &pipeline,
                                &bytecode,
                                &constants,
                                Some(entry),
                                name,
                                *offset,
                            );
                            if ok {
                                passed += 1;
                            } else {
                                failed += 1;
                                any_fail = true;
                                if fail_fast {
                                    stop = true;
                                    break;
                                }
                            }
                        }
                        !any_fail
                    }
                }
            }
        };

        if file_ok {
            eprintln!("ok   {display}");
        } else {
            eprintln!("FAILED {display}");
        }
    }

    Ok((passed, failed))
}

fn cmd_test(config: ReportConfig, path: Option<String>, fail_fast: bool) {
    let root = path.unwrap_or_else(|| TESTS_DIR.to_string());
    let tests_dir = Path::new(&root);
    let (passed, failed) = match run_test_suite(config.clone(), tests_dir, fail_fast) {
        Ok(counts) => counts,
        Err(msg) => {
            let format = config.format;
            let mut pipeline = Pipeline::with_reporter(config, writer_for(format));
            fail_and_exit(&mut pipeline, ErrorCode::IoError, msg);
        }
    };

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
    if let Some(panicked) = try_run_embedded() {
        exit(if panicked { 1 } else { 0 });
    }

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
        Command::Test { path, fail_fast } => cmd_test(config, path, fail_fast),
        command => {
            let format = config.format;
            let mut pipeline = Pipeline::with_reporter(config, writer_for(format));
            if cli.include_tests {
                pipeline.set_include_tests(true);
            }
            match command {
                Command::BuildAndRun { filename } => cmd_build_and_run(&mut pipeline, &filename),
                Command::Compile { filename, output } => {
                    cmd_compile(&mut pipeline, &filename, &output)
                }
                Command::Run { archive } => cmd_run(&mut pipeline, &archive),
                Command::Package {
                    filename,
                    output,
                    runner,
                    check_native,
                    strip_debug,
                } => cmd_package(
                    &mut pipeline,
                    &filename,
                    &output,
                    runner.as_deref(),
                    check_native,
                    strip_debug,
                ),
                Command::Test { .. } => unreachable!(),
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
    fn parse_package_default_output() {
        let cli = parse_args(&args(&["package", "examples/fib.0s"])).unwrap();
        assert_eq!(
            cli.command,
            Command::Package {
                filename: "examples/fib.0s".into(),
                output: "fib".into(),
                runner: None,
                check_native: false,
                strip_debug: false,
            }
        );
    }

    #[test]
    fn parse_package_with_flags() {
        let cli = parse_args(&args(&[
            "package",
            "app.0s",
            "-o",
            "myapp",
            "--check-native",
            "--strip-debug",
            "--runner",
            "/usr/bin/zero-script",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            Command::Package {
                filename: "app.0s".into(),
                output: "myapp".into(),
                runner: Some(PathBuf::from("/usr/bin/zero-script")),
                check_native: true,
                strip_debug: true,
            }
        );
    }

    #[test]
    fn parse_test() {
        let cli = parse_args(&args(&["test"])).unwrap();
        assert_eq!(
            cli.command,
            Command::Test {
                path: None,
                fail_fast: false,
            }
        );
    }

    #[test]
    fn parse_test_with_path_and_fail_fast() {
        let cli = parse_args(&args(&["test", "./tests", "--fail-fast"])).unwrap();
        assert_eq!(
            cli.command,
            Command::Test {
                path: Some("./tests".into()),
                fail_fast: true,
            }
        );
        let cli = parse_args(&args(&["--fail-fast", "test"])).unwrap();
        assert_eq!(
            cli.command,
            Command::Test {
                path: None,
                fail_fast: true,
            }
        );
    }

    #[test]
    fn parse_log_flags_with_subcommand() {
        let cli = parse_args(&args(&["--log-json", "compile", "a.0s"])).unwrap();
        assert!(cli.log_json);
        assert!(matches!(cli.command, Command::Compile { .. }));

        let cli = parse_args(&args(&["test", "--log-lsp"])).unwrap();
        assert!(cli.log_lsp);
        assert_eq!(
            cli.command,
            Command::Test {
                path: None,
                fail_fast: false,
            }
        );
    }

    #[test]
    fn parse_rejects_output_on_run_and_test() {
        assert!(parse_args(&args(&["run", "a.c0s", "-o", "x"])).is_err());
        assert!(parse_args(&args(&["test", "-o", "x"])).is_err());
        assert!(parse_args(&args(&["examples/fib.0s", "-o", "x"])).is_err());
        assert!(parse_args(&args(&["test", "--include-tests"])).is_err());
    }

    #[test]
    fn parse_rejects_missing_compile_file() {
        assert!(parse_args(&args(&["compile"])).is_err());
        assert!(parse_args(&args(&["run"])).is_err());
        assert!(parse_args(&args(&[])).is_err());
    }

    #[test]
    fn parse_rejects_fail_fast_on_non_test_commands() {
        assert!(parse_args(&args(&["--fail-fast", "examples/fib.0s"])).is_err());
        assert!(parse_args(&args(&["compile", "a.0s", "--fail-fast"])).is_err());
        assert!(parse_args(&args(&["run", "out.c0s", "--fail-fast"])).is_err());
    }

    #[test]
    fn parse_rejects_reserved_test_path_names() {
        assert!(parse_args(&args(&["test", "compile"])).is_err());
        assert!(parse_args(&args(&["test", "run"])).is_err());
        assert!(parse_args(&args(&["test", "test"])).is_err());
    }

    #[test]
    fn parse_rejects_unrecognized_flag() {
        assert!(parse_args(&args(&["--bogus", "a.0s"])).is_err());
    }

    #[test]
    fn parse_rejects_duplicate_output_and_missing_output_path() {
        assert!(parse_args(&args(&["compile", "a.0s", "-o"])).is_err());
        assert!(parse_args(&args(&["compile", "a.0s", "-o", "-x"])).is_err());
        assert!(parse_args(&args(&["compile", "a.0s", "-o", "x", "--output", "y"])).is_err());
    }

    #[test]
    fn parse_rejects_too_many_args_and_reserved_compile_names() {
        assert!(parse_args(&args(&["a.0s", "b.0s"])).is_err());
        assert!(parse_args(&args(&["compile", "compile"])).is_err());
        assert!(parse_args(&args(&["compile", "run"])).is_err());
        assert!(parse_args(&args(&["compile", "test"])).is_err());
    }

    #[test]
    fn parse_accepts_both_log_flags_at_parse_time() {
        // Mutual exclusion is enforced later by ReportConfig::from_cli_flags.
        let cli = parse_args(&args(&["--log-json", "--log-lsp", "test"])).unwrap();
        assert!(cli.log_json && cli.log_lsp);

        let cli = parse_args(&args(&["--include-tests", "examples/fib.0s"])).unwrap();
        assert!(cli.include_tests);
        assert!(matches!(cli.command, Command::BuildAndRun { .. }));
    }

    fn unique_tmp(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zero_script_cli_{label}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn try_load_archive_missing_corrupt_version_and_ok() {
        let missing = unique_tmp("missing");
        assert!(matches!(
            try_load_archive(missing.to_str().unwrap()),
            Err(LoadErr::Missing)
        ));

        let corrupt = unique_tmp("corrupt");
        std::fs::write(&corrupt, b"not-an-archive").unwrap();
        assert!(matches!(
            try_load_archive(corrupt.to_str().unwrap()),
            Err(LoadErr::Corrupt)
        ));
        let _ = std::fs::remove_file(&corrupt);

        let stale = unique_tmp("stale");
        let stale_version = if ARCHIVE_VERSION == 1 { 2 } else { 1 };
        let bytes = rkyv::to_bytes::<Error>(&ArchivedProgram {
            version: stale_version,
            static_slot_count: 0,
            constants: vec![],
            bytecode: vec![Byte::new(common::Instruction::HALT)],
            source_files: vec![],
            debug_locs: vec![common::DebugLoc::unknown()],
        })
        .unwrap();
        std::fs::write(&stale, bytes.as_slice()).unwrap();
        let loaded = try_load_archive(stale.to_str().unwrap());
        assert!(
            matches!(loaded, Err(LoadErr::Version(v)) if v == stale_version),
            "{loaded:?}"
        );
        let _ = std::fs::remove_file(&stale);

        let ok_path = unique_tmp("ok");
        let ok_prog = ArchivedProgram {
            version: ARCHIVE_VERSION,
            static_slot_count: 0,
            constants: vec![42],
            bytecode: vec![Byte::new(common::Instruction::HALT)],
            source_files: vec![],
            debug_locs: vec![common::DebugLoc::unknown()],
        };
        let ok_bytes = rkyv::to_bytes::<Error>(&ok_prog).unwrap();
        std::fs::write(&ok_path, ok_bytes.as_slice()).unwrap();
        let (bc, constants, _, _) = try_load_archive(ok_path.to_str().unwrap()).expect("ok archive");
        assert_eq!(constants, vec![42]);
        assert_eq!(bc.len(), 1);
        let _ = std::fs::remove_file(&ok_path);
    }

    #[test]
    fn source_newer_than_archive_compares_mtimes() {
        let dir = unique_tmp("mtime");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.0s");
        let arch = dir.join("a.c0s");
        // Archive first, then source after a short sleep so src mtime is newer.
        std::fs::write(&arch, b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(&src, b"fn main() {}").unwrap();
        assert!(source_newer_than_archive(
            src.to_str().unwrap(),
            arch.to_str().unwrap()
        ));
        // Missing paths => false
        assert!(!source_newer_than_archive(
            dir.join("nope.0s").to_str().unwrap(),
            arch.to_str().unwrap()
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_test_files_errors_and_discovers_nested() {
        let missing = unique_tmp("no_tests");
        assert!(collect_test_files(&missing).is_err());

        let empty = unique_tmp("empty_tests");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(collect_test_files(&empty).is_err());

        let root = unique_tmp("nested_tests");
        let nested = root.join("more");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("b.0s"), b"fn main() {}").unwrap();
        std::fs::write(nested.join("a.0s"), b"fn main() {}").unwrap();
        std::fs::write(root.join("ignore.txt"), b"x").unwrap();
        let files = collect_test_files(&root).expect("files");
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("a.0s") || files[0].ends_with("b.0s"));
        // Sorted lexicographically by full path.
        assert!(files[0] < files[1]);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn is_compile_fail_detects_path_segment() {
        assert!(is_compile_fail(Path::new("tests/compile_fail/bad.0s")));
        assert!(is_compile_fail(Path::new("/tmp/compile_fail/x.0s")));
        assert!(is_compile_fail(Path::new("suite/nested/compile_fail/deep/x.0s")));
        assert!(!is_compile_fail(Path::new("tests/arithmetic.0s")));
        assert!(!is_compile_fail(Path::new("tests/compile_fail_not/x.0s")));
        assert!(!is_compile_fail(Path::new("tests/my_compile_fail/x.0s")));
    }

    #[test]
    fn compile_fail_rejected_requires_clean_diagnostic_err() {
        let rejected_err: std::thread::Result<Result<(), ()>> = Ok(Err(()));
        assert!(compile_fail_rejected(&rejected_err));

        let unexpected_ok: std::thread::Result<Result<(), ()>> = Ok(Ok(()));
        assert!(!compile_fail_rejected(&unexpected_ok));

        // Panic is NOT a clean rejection (release panic=abort aborts anyway).
        let panicked: std::thread::Result<Result<(), ()>> = Err(Box::new("boom"));
        assert!(!compile_fail_rejected(&panicked));
    }

    #[test]
    fn run_test_suite_compile_fail_inversion_and_mixed_tree() {
        let root = unique_tmp("compile_fail_suite");
        let cf = root.join("compile_fail");
        let pos = root.join("positive");
        std::fs::create_dir_all(&cf).unwrap();
        std::fs::create_dir_all(&pos).unwrap();

        // Type error under compile_fail/ ⇒ harness pass.
        std::fs::write(
            cf.join("bad.0s"),
            "fn main() {\n  let x: int = \"no\";\n}\n",
        )
        .unwrap();
        // Well-typed under compile_fail/ ⇒ harness failure (inverted).
        std::fs::write(
            cf.join("unexpected_ok.0s"),
            "fn main() {\n  print \"%i\", 1;\n}\n",
        )
        .unwrap();
        // Normal positive case still runs.
        std::fs::write(
            pos.join("ok.0s"),
            "test(\"ok\") {\n  assert(true)?;\n}\n",
        )
        .unwrap();

        let (passed, failed) =
            run_test_suite(ReportConfig::default(), &root, false).expect("suite runs");
        assert_eq!(passed, 2, "bad compile_fail + positive ok");
        assert_eq!(failed, 1, "unexpected_ok under compile_fail must fail");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_test_suite_fail_fast_stops_after_unexpected_compile_ok() {
        let root = unique_tmp("compile_fail_fail_fast");
        let cf = root.join("compile_fail");
        std::fs::create_dir_all(&cf).unwrap();

        // Lexicographic order: a_ok before z_bad — fail-fast must stop after a_ok.
        std::fs::write(
            cf.join("a_ok.0s"),
            "fn main() {\n  print \"%i\", 1;\n}\n",
        )
        .unwrap();
        std::fs::write(
            cf.join("z_bad.0s"),
            "fn main() {\n  let x: int = \"no\";\n}\n",
        )
        .unwrap();

        let (passed, failed) =
            run_test_suite(ReportConfig::default(), &root, true).expect("suite runs");
        assert_eq!(failed, 1, "a_ok should fail (unexpected compile success)");
        assert_eq!(passed, 0, "fail-fast must not reach z_bad");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn archive_mtime_returns_none_for_missing() {
        assert!(archive_mtime(unique_tmp("no_mtime").to_str().unwrap()).is_none());
    }

    /// Fresh VM per case: soft-fail / panic in earlier cases must not skip later ones.
    #[test]
    fn harness_isolates_cases_and_continues_after_failures() {
        let src = r#"
test("soft fail") {
    assert(false)?;
}
test("panics") {
    panic "boom";
}
test("still runs") {
    assert(true)?;
}
"#;
        let mut pipeline = Pipeline::new();
        pipeline.set_include_tests(true);
        let (bytecode, constants) = pipeline
            .compile_src(src)
            .expect("multi-case harness source should compile");
        let cases = pipeline.test_cases().to_vec();
        assert_eq!(cases.len(), 3, "expected three test(\"…\") cases");
        assert_eq!(cases[0].0, "soft fail");
        assert_eq!(cases[1].0, "panics");
        assert_eq!(cases[2].0, "still runs");

        let mut passed = 0usize;
        let mut failed = 0usize;
        for (name, offset) in &cases {
            if run_test_case(&pipeline, &bytecode, &constants, None, name, *offset) {
                passed += 1;
            } else {
                failed += 1;
            }
        }
        assert_eq!(failed, 2, "soft-fail + panic should each count as failures");
        assert_eq!(passed, 1, "later case must still run after earlier failures");
    }
}
