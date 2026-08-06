//! `coil-debug` binary entry.

use std::process::exit;

use coil_debug::{DebugArgs, cmd_dap, cmd_debug};
use reporting::ReportConfig;

fn print_help() {
    eprintln!(
        "Usage:\n\
         \x20 coil-debug [--log-json | --log-lsp] [--dap] [<file.hy>] [-x <script>] [--batch]\n\
         \n\
         Options:\n\
         \x20 --dap         Debug Adapter Protocol over stdio (program from DAP launch)\n\
         \x20 -x <script>   Run commands from a script file\n\
         \x20 --batch       Non-interactive; exit after script / stdin\n\
         \x20 --log-json    Emit SARIF 2.1 diagnostics on stdout\n\
         \x20 --log-lsp     Emit LSP Diagnostic NDJSON on stdout\n\
         \x20 -h, --help    Show this help"
    );
}

fn parse_args(args: &[String]) -> Result<Option<(ReportConfig, DebugArgs)>, String> {
    let mut log_json = false;
    let mut log_lsp = false;
    let mut batch = false;
    let mut dap = false;
    let mut script: Option<String> = None;
    let mut filename: Option<String> = None;
    let mut i = 1usize;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-h" | "--help" => {
                print_help();
                exit(0);
            }
            "--dap" => dap = true,
            "--log-json" => log_json = true,
            "--log-lsp" => log_lsp = true,
            "--batch" => batch = true,
            "-x" => {
                i += 1;
                let path = args
                    .get(i)
                    .ok_or_else(|| "missing path after -x".to_string())?;
                script = Some(path.clone());
            }
            s if s.starts_with('-') => {
                return Err(format!("unrecognized flag `{s}`"));
            }
            _ => {
                if filename.is_some() {
                    return Err("unexpected extra argument".into());
                }
                filename = Some(a.clone());
            }
        }
        i += 1;
    }

    if dap {
        if filename.is_some() || script.is_some() || batch || log_json || log_lsp {
            return Err("--dap cannot be combined with REPL flags or a positional file".into());
        }
        return Ok(None);
    }

    let filename = filename.ok_or_else(|| "debug requires an entry .hy file".to_string())?;
    let config = ReportConfig::from_cli_flags(log_json, log_lsp).map_err(|e| e.to_string())?;
    Ok(Some((
        config,
        DebugArgs {
            filename,
            script,
            batch,
        },
    )))
}

fn main() {
    let raw: Vec<String> = std::env::args().collect();
    match parse_args(&raw) {
        Ok(None) => cmd_dap(),
        Ok(Some((config, args))) => cmd_debug(config, args),
        Err(msg) => {
            eprintln!("coil-debug: {msg}");
            print_help();
            exit(1);
        }
    }
}
