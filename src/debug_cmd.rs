//! `coil debug` — GDB-style interactive / scripted debugger.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::exit;

use common::{ProgramDebug, byte_to_position};
use compiler::{DissectArtifacts, FnSym, Pipeline, format_bytecode_section, matches_fn_pat};
use machine::{DebugController, Machine, StopReason};
use reporting::ReportConfig;

use crate::writer_for;

pub struct DebugArgs {
    pub filename: String,
    pub script: Option<String>,
    pub batch: bool,
}

#[derive(Clone, Debug)]
struct Breakpoint {
    id: usize,
    pc: usize,
    label: String,
}

/// Line → PCs index for one source file (path as stored in ProgramDebug).
#[derive(Default)]
struct LineIndex {
    /// canonical path string → line → pcs
    by_file: HashMap<String, HashMap<u32, Vec<usize>>>,
    /// basename → full path keys (for `break 3` / short names)
    by_basename: HashMap<String, String>,
}

impl LineIndex {
    fn build(debug: &ProgramDebug, base_dir: Option<&Path>) -> Self {
        let mut idx = LineIndex::default();
        let mut texts: HashMap<u32, String> = HashMap::new();
        for (pc, loc) in debug.debug_locs.iter().enumerate() {
            if !loc.is_known() {
                continue;
            }
            let path = match debug.source_files.get(loc.file as usize) {
                Some(p) => p.clone(),
                None => continue,
            };
            let text = texts.entry(loc.file).or_insert_with(|| {
                let resolved = resolve_path(&path, base_dir);
                fs::read_to_string(resolved).unwrap_or_default()
            });
            if text.is_empty() {
                continue;
            }
            let line = byte_to_position(text, loc.start_byte as usize).line;
            idx.by_file
                .entry(path.clone())
                .or_default()
                .entry(line)
                .or_default()
                .push(pc);
            if let Some(base) = Path::new(&path).file_name().and_then(|s| s.to_str()) {
                idx.by_basename
                    .entry(base.to_string())
                    .or_insert_with(|| path.clone());
            }
        }
        idx
    }

    fn pcs_for_line(&self, file_hint: Option<&str>, line: u32, entry_file: &str) -> Vec<usize> {
        let key = if let Some(hint) = file_hint {
            if self.by_file.contains_key(hint) {
                hint.to_string()
            } else if let Some(full) = self.by_basename.get(hint) {
                full.clone()
            } else {
                // try suffix match
                self.by_file
                    .keys()
                    .find(|k| k.ends_with(hint) || Path::new(k).ends_with(hint))
                    .cloned()
                    .unwrap_or_else(|| entry_file.to_string())
            }
        } else {
            entry_file.to_string()
        };
        self.by_file
            .get(&key)
            .and_then(|m| m.get(&line))
            .cloned()
            .unwrap_or_default()
    }
}

fn resolve_path(path: &str, base_dir: Option<&Path>) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() || p.exists() {
        return p;
    }
    if let Some(base) = base_dir {
        let root = base.parent().unwrap_or(base);
        let from_root = root.join(path);
        if from_root.exists() {
            return from_root;
        }
        let from_base = base.join(path);
        if from_base.exists() {
            return from_base;
        }
    }
    p
}

struct DebugSession {
    entry: String,
    artifacts: DissectArtifacts,
    static_slots: u32,
    line_index: LineIndex,
    machine: Machine<256>,
    breakpoints: Vec<Breakpoint>,
    next_bp_id: usize,
    /// True after a successful `run` until halt/quit; continue resumes.
    started: bool,
    base_dir: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
enum CmdResult {
    ContinuePrompt,
    Quit,
}

pub fn cmd_debug(config: ReportConfig, args: DebugArgs) {
    let format = config.format;
    let mut pipeline = Pipeline::with_reporter(config, writer_for(format));

    let artifacts = match pipeline.compile_dissect(&args.filename, false) {
        Ok(a) => a,
        Err(()) => {
            let _ = pipeline.finish_reporting();
            exit(1);
        }
    };
    let static_slots = pipeline.static_slot_count();
    let entry_path = PathBuf::from(&args.filename);
    let base_dir = entry_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let line_index = LineIndex::build(&artifacts.debug, Some(&base_dir));

    let mut machine = Machine::<256>::default();
    pipeline.wire_vm_ffi(&mut machine, Some(&entry_path));
    pipeline.wire_host_natives(&mut machine);
    pipeline.wire_thread_program(
        &mut machine,
        &artifacts.bytecode,
        &artifacts.constants,
        &artifacts.strings,
    );
    machine.set_program_debug(artifacts.debug.clone());
    machine.attach_debug(DebugController::new());

    let _ = pipeline.finish_reporting();

    let mut session = DebugSession {
        entry: args.filename.clone(),
        artifacts,
        static_slots,
        line_index,
        machine,
        breakpoints: Vec::new(),
        next_bp_id: 1,
        started: false,
        base_dir,
    };

    let batch = args.batch;
    let mut script_lines: Vec<String> = Vec::new();
    if let Some(ref path) = args.script {
        match fs::read_to_string(path) {
            Ok(s) => {
                for line in s.lines() {
                    script_lines.push(line.to_string());
                }
            }
            Err(e) => {
                eprintln!("debug: failed to read script `{path}`: {e}");
                exit(1);
            }
        }
    } else if batch {
        // --batch with no -x: read commands from stdin once.
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => script_lines.push(l),
                Err(e) => {
                    eprintln!("debug: stdin read error: {e}");
                    exit(1);
                }
            }
        }
    }

    for line in &script_lines {
        match exec_line(&mut session, line, batch) {
            Ok(CmdResult::Quit) => {
                exit(if session.machine.panicked() { 1 } else { 0 });
            }
            Ok(CmdResult::ContinuePrompt) => {}
            Err(e) => {
                eprintln!("debug: {e}");
                if batch {
                    exit(1);
                }
            }
        }
    }

    if batch {
        exit(if session.machine.panicked() { 1 } else { 0 });
    }

    // Interactive REPL
    let stdin = io::stdin();
    loop {
        eprint!("(coil) ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("debug: read error: {e}");
                break;
            }
        }
        match exec_line(&mut session, &line, false) {
            Ok(CmdResult::Quit) => break,
            Ok(CmdResult::ContinuePrompt) => {}
            Err(e) => eprintln!("debug: {e}"),
        }
    }
    if session.machine.panicked() {
        exit(1);
    }
}

fn exec_line(session: &mut DebugSession, line: &str, batch: bool) -> Result<CmdResult, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(CmdResult::ContinuePrompt);
    }
    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();

    match cmd {
        "help" | "h" => {
            print_help();
            Ok(CmdResult::ContinuePrompt)
        }
        "quit" | "q" => Ok(CmdResult::Quit),
        "break" | "b" => {
            let arg = rest
                .first()
                .copied()
                .ok_or("usage: break <fn|file:line|line>")?;
            cmd_break(session, arg)?;
            Ok(CmdResult::ContinuePrompt)
        }
        "delete" | "d" => {
            if rest.is_empty() {
                session.breakpoints.clear();
                sync_vm_breakpoints(session);
                println!("Deleted all breakpoints.");
            } else {
                let id: usize = rest[0]
                    .parse()
                    .map_err(|_| format!("invalid breakpoint id `{}`", rest[0]))?;
                let before = session.breakpoints.len();
                session.breakpoints.retain(|b| b.id != id);
                if session.breakpoints.len() == before {
                    return Err(format!("no breakpoint {id}"));
                }
                sync_vm_breakpoints(session);
                println!("Deleted breakpoint {id}.");
            }
            Ok(CmdResult::ContinuePrompt)
        }
        "info" => {
            let sub = rest.first().copied().unwrap_or("");
            match sub {
                "break" | "breakpoints" => {
                    if session.breakpoints.is_empty() {
                        println!("No breakpoints.");
                    } else {
                        println!("Num  Pc    What");
                        for b in &session.breakpoints {
                            println!("{:<4} {:<5} {}", b.id, b.pc, b.label);
                        }
                    }
                }
                "registers" | "reg" => {
                    let ip = session.machine.debug_ip();
                    let depth = session.machine.debug_frame_depth();
                    let sp = session
                        .machine
                        .debug_frame_sp(depth.saturating_sub(1))
                        .unwrap_or(0);
                    println!("ip={ip}  sp={sp}  depth={depth}");
                }
                "locals" => {
                    cmd_info_locals(session)?;
                }
                _ => return Err("usage: info break | info registers | info locals".into()),
            }
            Ok(CmdResult::ContinuePrompt)
        }
        "run" | "r" => {
            cmd_run(session)?;
            Ok(CmdResult::ContinuePrompt)
        }
        "continue" | "c" => {
            if !session.started {
                return Err("not started; use `run` first".into());
            }
            prepare_resume(session);
            let reason = session.machine.debug_run_until_raw(
                &session.artifacts.bytecode,
                &session.artifacts.constants,
                &session.artifacts.strings,
                session.static_slots,
                session.machine.debug_ip(),
            );
            report_stop(session, &reason);
            if matches!(reason, StopReason::Halt | StopReason::Panic) {
                session.started = false;
                if batch && matches!(reason, StopReason::Panic) {
                    return Err("program panicked".into());
                }
            }
            Ok(CmdResult::ContinuePrompt)
        }
        "stepi" | "si" => {
            cmd_stepi(session, batch)?;
            Ok(CmdResult::ContinuePrompt)
        }
        "step" | "s" => {
            cmd_step_line(session, false, batch)?;
            Ok(CmdResult::ContinuePrompt)
        }
        "next" | "n" => {
            cmd_step_line(session, true, batch)?;
            Ok(CmdResult::ContinuePrompt)
        }
        "finish" | "fin" => {
            cmd_finish(session, batch)?;
            Ok(CmdResult::ContinuePrompt)
        }
        "print" | "p" => {
            let arg = rest.first().copied().ok_or("usage: print <name|$N>")?;
            cmd_print(session, arg)?;
            Ok(CmdResult::ContinuePrompt)
        }
        "bt" | "backtrace" => {
            cmd_bt(session);
            Ok(CmdResult::ContinuePrompt)
        }
        "list" | "l" => {
            cmd_list(session)?;
            Ok(CmdResult::ContinuePrompt)
        }
        "disassemble" | "disas" | "dis" => {
            let pat = rest.first().copied();
            cmd_disas(session, pat)?;
            Ok(CmdResult::ContinuePrompt)
        }
        _ => Err(format!("unknown command `{cmd}` (try `help`)")),
    }
}

fn print_help() {
    println!(
        "Commands:\n\
         \x20 break / b <fn|file:line|line>  Set breakpoint\n\
         \x20 delete / d [n]                 Delete breakpoint(s)\n\
         \x20 info break | info registers    Status\n\
         \x20 run / r                        Start or restart\n\
         \x20 continue / c                   Resume\n\
         \x20 stepi / si                     Step one bytecode insn\n\
         \x20 step / s                       Step to next source line (into)\n\
         \x20 next / n                       Step over (same/outer depth)\n\
         \x20 finish / fin                   Run until frame returns\n\
         \x20 print / p <name|$N>           Print local by name or slot\n\
         \x20 info locals                   List named locals in current frame\n\
         \x20 bt / backtrace                 Call stack\n\
         \x20 list / l                       Source around stop\n\
         \x20 disassemble / disas [fn]       Bytecode dump\n\
         \x20 help / h                       This help\n\
         \x20 quit / q                       Exit"
    );
}

fn sync_vm_breakpoints(session: &mut DebugSession) {
    if let Some(dbg) = session.machine.debug_controller_mut() {
        dbg.clear_breakpoints();
        for b in &session.breakpoints {
            dbg.add_breakpoint(b.pc);
        }
    }
}

fn cmd_break(session: &mut DebugSession, arg: &str) -> Result<(), String> {
    let (pcs, label) = resolve_break_target(session, arg)?;
    if pcs.is_empty() {
        return Err(format!("no code locations for `{arg}`"));
    }
    let pc = pcs[0];
    let id = session.next_bp_id;
    session.next_bp_id += 1;
    let label = format!("{label} (pc {pc})");
    println!("Breakpoint {id} at {label}");
    session.breakpoints.push(Breakpoint { id, pc, label });
    sync_vm_breakpoints(session);
    Ok(())
}

fn resolve_break_target(session: &DebugSession, arg: &str) -> Result<(Vec<usize>, String), String> {
    // file:line
    if let Some((file, line_s)) = arg.rsplit_once(':')
        && !file.is_empty()
        && line_s.chars().all(|c| c.is_ascii_digit())
    {
        let line: u32 = line_s.parse().map_err(|_| "invalid line number")?;
        let pcs = session
            .line_index
            .pcs_for_line(Some(file), line, &session.entry);
        return Ok((pcs, format!("{file}:{line}")));
    }
    // bare line
    if arg.chars().all(|c| c.is_ascii_digit()) {
        let line: u32 = arg.parse().map_err(|_| "invalid line number")?;
        let pcs = session.line_index.pcs_for_line(None, line, &session.entry);
        return Ok((pcs, format!("{}:{line}", session.entry)));
    }
    // function name
    let matched: Vec<&FnSym> = session
        .artifacts
        .functions
        .iter()
        .filter(|s| matches_fn_pat(&s.name, arg))
        .collect();
    if matched.is_empty() {
        return Err(format!("no function matching `{arg}`"));
    }
    let pcs: Vec<usize> = matched.iter().map(|s| s.entry_pc as usize).collect();
    let name = matched[0].name.clone();
    Ok((pcs, name))
}

fn cmd_run(session: &mut DebugSession) -> Result<(), String> {
    session.machine.debug_reset();
    // Re-attach empty step state; breakpoints re-synced below.
    if session.machine.debug_controller().is_none() {
        session.machine.attach_debug(DebugController::new());
    }
    sync_vm_breakpoints(session);
    session.started = true;
    let reason = session.machine.debug_run_until_raw(
        &session.artifacts.bytecode,
        &session.artifacts.constants,
        &session.artifacts.strings,
        session.static_slots,
        0,
    );
    report_stop(session, &reason);
    if matches!(reason, StopReason::Halt | StopReason::Panic) {
        session.started = false;
    }
    if matches!(reason, StopReason::Panic) {
        return Err("program panicked".into());
    }
    Ok(())
}

fn prepare_resume(session: &mut DebugSession) {
    let ip = session.machine.debug_ip();
    if let Some(dbg) = session.machine.debug_controller_mut() {
        dbg.clear_step();
        if dbg.breakpoints().contains(&ip) {
            dbg.skip_breakpoint_once(ip);
        }
    }
}

fn cmd_stepi(session: &mut DebugSession, batch: bool) -> Result<(), String> {
    if !session.started {
        return Err("not started; use `run` first".into());
    }
    let ip = session.machine.debug_ip();
    if let Some(dbg) = session.machine.debug_controller_mut() {
        dbg.set_stepi();
        if dbg.breakpoints().contains(&ip) {
            dbg.skip_breakpoint_once(ip);
        }
    }
    let reason = session.machine.debug_run_until_raw(
        &session.artifacts.bytecode,
        &session.artifacts.constants,
        &session.artifacts.strings,
        session.static_slots,
        ip,
    );
    report_stop(session, &reason);
    if matches!(reason, StopReason::Halt | StopReason::Panic) {
        session.started = false;
        if batch && matches!(reason, StopReason::Panic) {
            return Err("program panicked".into());
        }
    }
    Ok(())
}

fn cmd_step_line(session: &mut DebugSession, next: bool, batch: bool) -> Result<(), String> {
    if !session.started {
        return Err("not started; use `run` first".into());
    }
    let ip = session.machine.debug_ip();
    let depth = session.machine.debug_frame_depth();
    let (file, line) = match session.machine.debug_pc_line(ip) {
        Some(fl) => fl,
        None => {
            // No source loc — fall back to stepi.
            return cmd_stepi(session, batch);
        }
    };
    if let Some(dbg) = session.machine.debug_controller_mut() {
        if next {
            dbg.set_next(file, line, depth);
        } else {
            dbg.set_step_line(file, line, depth);
        }
        if dbg.breakpoints().contains(&ip) {
            dbg.skip_breakpoint_once(ip);
        }
    }
    let reason = session.machine.debug_run_until_raw(
        &session.artifacts.bytecode,
        &session.artifacts.constants,
        &session.artifacts.strings,
        session.static_slots,
        ip,
    );
    report_stop(session, &reason);
    if matches!(reason, StopReason::Halt | StopReason::Panic) {
        session.started = false;
        if batch && matches!(reason, StopReason::Panic) {
            return Err("program panicked".into());
        }
    }
    Ok(())
}

fn cmd_finish(session: &mut DebugSession, batch: bool) -> Result<(), String> {
    if !session.started {
        return Err("not started; use `run` first".into());
    }
    let ip = session.machine.debug_ip();
    let depth = session.machine.debug_frame_depth();
    if depth == 0 {
        return Err("no frame to finish".into());
    }
    if let Some(dbg) = session.machine.debug_controller_mut() {
        dbg.set_finish(depth - 1);
        if dbg.breakpoints().contains(&ip) {
            dbg.skip_breakpoint_once(ip);
        }
    }
    let reason = session.machine.debug_run_until_raw(
        &session.artifacts.bytecode,
        &session.artifacts.constants,
        &session.artifacts.strings,
        session.static_slots,
        ip,
    );
    report_stop(session, &reason);
    if matches!(reason, StopReason::Halt | StopReason::Panic) {
        session.started = false;
        if batch && matches!(reason, StopReason::Panic) {
            return Err("program panicked".into());
        }
    }
    Ok(())
}

fn locals_for_pc<'a>(session: &'a DebugSession, pc: usize) -> Option<&'a [(String, u32)]> {
    let name = symbol_at_pc(&session.artifacts.functions, pc)?;
    session
        .artifacts
        .functions
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.locals.as_slice())
}

fn resolve_local_slot(session: &DebugSession, name: &str) -> Result<usize, String> {
    let ip = session.machine.debug_ip();
    let locals = locals_for_pc(session, ip).unwrap_or(&[]);
    let name_l = name.to_ascii_lowercase();
    for (n, slot) in locals {
        if n.eq_ignore_ascii_case(name) || n.to_ascii_lowercase() == name_l {
            return Ok(*slot as usize);
        }
    }
    // Substring match when unique
    let matches: Vec<_> = locals
        .iter()
        .filter(|(n, _)| n.to_ascii_lowercase().contains(&name_l))
        .collect();
    match matches.as_slice() {
        [(_, slot)] => Ok(*slot as usize),
        [] => Err(format!(
            "no local `{name}` in current frame (try `info locals` or `print $N`)"
        )),
        many => Err(format!(
            "ambiguous local `{name}`: {}",
            many.iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn cmd_print(session: &DebugSession, arg: &str) -> Result<(), String> {
    let slot = if let Some(n) = arg.strip_prefix('$') {
        n.parse()
            .map_err(|_| format!("invalid slot `{arg}` (usage: print <name|$N>)"))?
    } else {
        resolve_local_slot(session, arg)?
    };
    let depth = session.machine.debug_frame_depth();
    if depth == 0 {
        return Err("no active frame".into());
    }
    let frame = depth - 1;
    let val = session
        .machine
        .debug_slot(frame, slot)
        .ok_or_else(|| format!("slot ${slot} out of range"))?;
    let label = locals_for_pc(session, session.machine.debug_ip())
        .and_then(|locals| {
            locals
                .iter()
                .find(|(_, s)| *s as usize == slot)
                .map(|(n, _)| n.as_str())
        })
        .unwrap_or("");
    if label.is_empty() {
        println!("${slot} = {}", session.machine.debug_format_value(val));
    } else {
        println!(
            "{label} (${slot}) = {}",
            session.machine.debug_format_value(val)
        );
    }
    Ok(())
}

fn cmd_info_locals(session: &DebugSession) -> Result<(), String> {
    if !session.started {
        return Err("not started; use `run` first".into());
    }
    let ip = session.machine.debug_ip();
    let depth = session.machine.debug_frame_depth();
    if depth == 0 {
        return Err("no active frame".into());
    }
    let frame = depth - 1;
    let fn_name = symbol_at_pc(&session.artifacts.functions, ip).unwrap_or("<unknown>");
    let locals = locals_for_pc(session, ip).unwrap_or(&[]);
    if locals.is_empty() {
        println!("No named locals for {fn_name}.");
        return Ok(());
    }
    println!("Locals of {fn_name}:");
    for (name, slot) in locals {
        let val = session
            .machine
            .debug_slot(frame, *slot as usize)
            .map(|v| session.machine.debug_format_value(v))
            .unwrap_or_else(|| "<unavailable>".into());
        println!("  {name} (${slot}) = {val}");
    }
    Ok(())
}

fn cmd_bt(session: &DebugSession) {
    let depth = session.machine.debug_frame_depth();
    if depth == 0 {
        println!("No stack.");
        return;
    }
    for i in (0..depth).rev() {
        let ip = session.machine.debug_frame_ip(i).unwrap_or(0);
        let sym = symbol_at_pc(&session.artifacts.functions, ip);
        let loc = session
            .machine
            .resolve_pc_location(ip)
            .map(|(p, l, c)| format!(" at {p}:{l}:{c}"))
            .unwrap_or_default();
        let name = sym.unwrap_or("<unknown>");
        println!("#{:<2} {} pc={}{}", depth - 1 - i, name, ip, loc);
    }
}

fn symbol_at_pc(functions: &[FnSym], pc: usize) -> Option<&str> {
    let mut best: Option<&FnSym> = None;
    for s in functions {
        let entry = s.entry_pc as usize;
        if entry <= pc && best.map(|b| entry >= b.entry_pc as usize).unwrap_or(true) {
            best = Some(s);
        }
    }
    best.map(|s| s.name.as_str())
}

fn cmd_list(session: &DebugSession) -> Result<(), String> {
    let ip = if session.started {
        session.machine.debug_ip()
    } else {
        return Err("not started; use `run` first".into());
    };
    let (path, line, _) = session
        .machine
        .resolve_pc_location(ip)
        .ok_or("no source location at current PC")?;
    let resolved = resolve_path(&path, Some(&session.base_dir));
    let text = fs::read_to_string(&resolved)
        .map_err(|e| format!("cannot read {}: {e}", resolved.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = line.saturating_sub(5).max(1);
    let end = (line + 5).min(lines.len() as u32);
    println!("{}:{}", resolved.display(), line);
    for n in start..=end {
        let mark = if n == line { '>' } else { ' ' };
        let src = lines.get((n - 1) as usize).unwrap_or(&"");
        println!("{mark}{n:4}  {src}");
    }
    Ok(())
}

fn cmd_disas(session: &DebugSession, pat: Option<&str>) -> Result<(), String> {
    let arts = &session.artifacts;
    let pc_names: HashMap<usize, &str> = arts
        .functions
        .iter()
        .map(|s| (s.entry_pc as usize, s.name.as_str()))
        .collect();
    if let Some(p) = pat {
        let matched: Vec<_> = arts
            .functions
            .iter()
            .filter(|s| matches_fn_pat(&s.name, p))
            .collect();
        if matched.is_empty() {
            return Err(format!("no function matching `{p}`"));
        }
        let mut syms = arts.functions.clone();
        syms.sort_by_key(|s| s.entry_pc);
        let len = arts.bytecode.len();
        for (i, sym) in syms.iter().enumerate() {
            if !matches_fn_pat(&sym.name, p) {
                continue;
            }
            let start = sym.entry_pc as usize;
            let end = syms
                .get(i + 1)
                .map(|n| n.entry_pc as usize)
                .unwrap_or(len)
                .min(len);
            print!(
                "{}",
                format_bytecode_section(
                    &sym.name,
                    start,
                    end.max(start),
                    &arts.bytecode,
                    &arts.constants,
                    &pc_names,
                )
            );
        }
    } else {
        let ip = session.machine.debug_ip();
        let start = ip.saturating_sub(4);
        let end = (ip + 12).min(arts.bytecode.len());
        print!(
            "{}",
            format_bytecode_section(
                &format!("pc={ip}"),
                start,
                end,
                &arts.bytecode,
                &arts.constants,
                &pc_names,
            )
        );
    }
    Ok(())
}

fn report_stop(session: &DebugSession, reason: &StopReason) {
    let ip = session.machine.debug_ip();
    let sym = symbol_at_pc(&session.artifacts.functions, ip).unwrap_or("<prog>");
    let loc = session
        .machine
        .resolve_pc_location(ip)
        .map(|(p, l, _)| format!(" at {p}:{l}"))
        .unwrap_or_default();
    match reason {
        StopReason::Breakpoint { pc } => {
            let id = session
                .breakpoints
                .iter()
                .find(|b| b.pc == *pc)
                .map(|b| b.id)
                .unwrap_or(0);
            println!("Breakpoint {id}, {sym}{loc} (pc {pc})");
        }
        StopReason::Step => println!("Step, {sym}{loc} (pc {ip})"),
        StopReason::Next => println!("Next, {sym}{loc} (pc {ip})"),
        StopReason::Finish => println!("Finish, {sym}{loc} (pc {ip})"),
        StopReason::Halt => println!("Program exited normally."),
        StopReason::Panic => println!("Program panicked."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::DebugLoc;

    #[test]
    fn line_index_maps_known_loc() {
        let text = "fn main() {\n    return;\n}\n";
        let tmp = std::env::temp_dir().join(format!("coil_dbg_line_{}", std::process::id()));
        fs::write(&tmp, text).unwrap();
        let path = tmp.display().to_string();
        let mut debug = ProgramDebug {
            source_files: vec![path.clone()],
            debug_locs: vec![DebugLoc::unknown(); 5],
        };
        let ret_off = text.find("return").unwrap() as u32;
        debug.debug_locs[3] = DebugLoc {
            file: 0,
            start_byte: ret_off,
            end_byte: ret_off + 6,
        };
        let idx = LineIndex::build(&debug, None);
        let pcs = idx.pcs_for_line(Some(&path), 2, &path);
        assert!(pcs.contains(&3), "pcs={pcs:?}");
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn parse_break_line_and_fn_shapes() {
        assert!("12".chars().all(|c| c.is_ascii_digit()));
        let (file, line) = "fib.hy:3".rsplit_once(':').unwrap();
        assert_eq!(file, "fib.hy");
        assert_eq!(line, "3");
    }
}
