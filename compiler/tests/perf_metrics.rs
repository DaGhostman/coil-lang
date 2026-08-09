//! Bytecode-shape and dispatch-count regression guards for the VM perf pass.

use common::{Byte, FnDebugSym, Instruction};
use compiler::Pipeline;
use machine::{Machine, dispatch_count, reset_dispatch_count};

fn compile(path: &str) -> (Vec<Byte>, Vec<u64>, Vec<String>, u32, Pipeline) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let src =
        std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(&src)
        .unwrap_or_else(|_| panic!("compile failed: {path}"));
    (
        bytecode,
        constants,
        pipeline.strings().to_vec(),
        pipeline.static_slot_count(),
        pipeline,
    )
}

fn count_opcodes(bytecode: &[Byte], op: Instruction) -> usize {
    bytecode.iter().filter(|b| *b.bytecode() == op).count()
}

/// Inclusive-exclusive PC range for `name` from sorted `fn_symbols`.
fn fn_pc_range(syms: &[FnDebugSym], name: &str, bytecode_len: usize) -> (usize, usize) {
    let idx = syms.iter().position(|s| s.name == name).unwrap_or_else(|| {
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        panic!("missing fn_symbol `{name}`; have {names:?}");
    });
    let start = syms[idx].entry_pc as usize;
    let end = syms
        .get(idx + 1)
        .map(|s| s.entry_pc as usize)
        .unwrap_or(bytecode_len);
    (start, end)
}

fn count_opcodes_in(bytecode: &[Byte], start: usize, end: usize, op: Instruction) -> usize {
    bytecode[start..end]
        .iter()
        .filter(|b| *b.bytecode() == op)
        .count()
}

fn run_dispatch(
    bytecode: Vec<Byte>,
    constants: Vec<u64>,
    strings: Vec<String>,
    static_slots: u32,
    pipeline: &Pipeline,
) -> u64 {
    reset_dispatch_count();
    let mut machine = Machine::<256>::default();
    // write_all / stdout path needs host natives (print opcode retired).
    pipeline.wire_host_natives(&mut machine);
    machine.run_raw(&bytecode, &constants, &strings, static_slots);
    dispatch_count()
}

#[test]
fn perf_tak_direct_calls_no_call_indirect() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/tak.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "tak", bc.len());
    let call_indirect = count_opcodes_in(&bc, start, end, Instruction::CallIndirect);
    let calls = count_opcodes_in(&bc, start, end, Instruction::CALL);
    let tails = count_opcodes_in(&bc, start, end, Instruction::TailCall);
    assert_eq!(
        call_indirect, 0,
        "tak must stay on direct CALL/TailCall (got CallIndirect={call_indirect})"
    );
    assert!(
        calls >= 3,
        "tak recursive arms should use CALL; got {calls}"
    );
    assert!(
        tails >= 1,
        "tak outer recursion should TailCall; got {tails}"
    );
    // Self-recursive peels: each nested CALL is guarded by a fused compare+branch.
    let body = &bc[start..end];
    let peel_guards = body
        .iter()
        .filter(|b| {
            matches!(
                *b.bytecode(),
                Instruction::BinSlotSlotJmpf
                    | Instruction::BinSlotImmJmpf
                    | Instruction::CmpJmpf
                    | Instruction::JMPF
            )
        })
        .count();
    assert!(
        peel_guards >= 4,
        "tak should keep entry guard + 3 self-peels; guards={peel_guards}; ops={:?}",
        body.iter().map(|b| b.bytecode()).collect::<Vec<_>>()
    );
}

#[test]
fn perf_tak_dispatch_regression() {
    let (bc, pool, strings, statics, pipeline) = compile("examples/perf/tak.hy");
    let dispatches = run_dispatch(bc, pool, strings, statics, &pipeline);
    // tak(18,12,6) is deep recursion; peels skip base-case frames.
    // Measured ~1.5–3M dispatches on debug Machine; keep headroom.
    assert!(
        dispatches < 4_000_000,
        "tak dispatch count regressed: {dispatches}"
    );
}

#[test]
fn perf_numeric_uses_bin_slot_imm_jmpf_for_loop() {
    let (bc, _, _, _, _) = compile("examples/perf/numeric.hy");
    assert!(
        count_opcodes(&bc, Instruction::BinSlotImmJmpf) >= 1,
        "numeric loop should fuse compare+branch"
    );
}

#[test]
fn perf_operators_loop_inverts_not_into_bin_slot_jmpf() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/operators_loop.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "main", bc.len());
    let main = &bc[start..end];
    // Stdlib (`io::sync::write_all`, …) may emit LogNotJmpf; the user loop must not.
    assert_eq!(
        count_opcodes_in(&bc, start, end, Instruction::LogNotJmpf),
        0,
        "main should not emit LogNotJmpf after if(!c) invert"
    );
    assert!(
        main.iter().any(|b| {
            matches!(*b.bytecode(), Instruction::BinSlotImmJmpf)
                && b.bin_slot_imm_jmpf_parts().0 == Instruction::BITAND as u8
        }),
        "operators loop should fuse BITAND into BinSlotImmJmpf"
    );
}

#[test]
fn perf_numeric_dispatch_regression() {
    let (bc, pool, strings, statics, pipeline) = compile("examples/perf/numeric.hy");
    let dispatches = run_dispatch(bc, pool, strings, statics, &pipeline);
    // Release of VM perf pass: loop compare+branch fused; expect well under 80k.
    assert!(
        dispatches < 80_000,
        "numeric dispatch count regressed: {dispatches}"
    );
}

#[test]
fn perf_match_sum_emits_jump_if_match() {
    let (bc, _, _, _, _) = compile("examples/perf/match_sum.hy");
    assert!(
        count_opcodes(&bc, Instruction::JumpIfMatch) >= 1,
        "match_sum should emit match dispatch"
    );
}

#[test]
fn perf_mandelbrot_dispatch_regression() {
    let (bc, pool, strings, statics, pipeline) = compile("examples/perf/mandelbrot.hy");
    let dispatches = run_dispatch(bc, pool, strings, statics, &pipeline);
    // Nested float loops (size=160, max_iter=50) + write_all.
    // Measured ~12M dispatches on debug Machine; keep headroom for stdlib churn.
    assert!(
        dispatches < 25_000_000,
        "mandelbrot dispatch count regressed: {dispatches}"
    );
}

#[test]
fn perf_field_hot_reuses_repeated_string_keys() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/field_hot.hy");
    let syms = pipeline.program_debug().fn_symbols;
    // Count STRING only in Point methods + main — not linked Show/String/io helpers.
    let mut strings = 0usize;
    for name in ["Point::sum", "Point::twice_x", "main"] {
        let (start, end) = fn_pc_range(&syms, name, bc.len());
        strings += count_opcodes_in(&bc, start, end, Instruction::STRING);
    }
    // Field keys "x"/"y" reused across methods/main; a few format/literals in main.
    assert!(
        strings <= 10,
        "field_hot user fns should reuse field-name STRINGs, got {strings}"
    );
    assert!(
        count_opcodes(&bc, Instruction::GetField) >= 1,
        "field_hot should emit GetField"
    );
}

#[test]
fn perf_for_in_array_uses_single_array_len() {
    let (bc, _, _, _, _) = compile("examples/for_in_array.hy");
    // Loop hoist emits one ArrayLen; `io::sync` helpers linked via write_all
    // contribute additional ArrayLen ops in the same archive.
    let n = count_opcodes(&bc, Instruction::ArrayLen);
    assert!(
        (1..=8).contains(&n),
        "for_in_array should hoist ArrayLen out of the loop (got {n})"
    );
}

#[test]
fn perf_indexed_sum_hoists_array_len_once() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/indexed_sum.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "sum", bc.len());
    let n = count_opcodes_in(&bc, start, end, Instruction::ArrayLen);
    assert_eq!(
        n, 1,
        "sum should hoist ArrayLen out of the while i < len(arr) loop (got {n})"
    );
    // ArrayLen must not sit on the back-edge cycle: find the loop JMP and
    // ensure its target PC is at-or-after the sole ArrayLen (preheader).
    let sum = &bc[start..end];
    let len_pc = sum
        .iter()
        .position(|b| *b.bytecode() == Instruction::ArrayLen)
        .expect("ArrayLen in sum");
    let back_edge = sum.iter().rposition(|b| *b.bytecode() == Instruction::JMP);
    let Some(be) = back_edge else {
        panic!("sum should have a back-edge JMP");
    };
    let target = sum[be].operand_u32() as usize;
    let target_rel = target.saturating_sub(start);
    assert!(
        len_pc < target_rel,
        "ArrayLen at {len_pc} must be before back-edge target {target_rel} (hoisted preheader)"
    );
    let stats = compiler::last_bounds_stats();
    assert!(
        stats.array_len_hoists >= 1,
        "indexed_sum should hoist ArrayLen; stats={stats:?}"
    );
    assert!(
        stats.proven_index >= 1,
        "indexed_sum Index under i < len should be proven; stats={stats:?}"
    );
}

#[test]
fn perf_nsieve_proves_fill_bounded_index() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/nsieve.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "nsieve", bc.len());
    // No UncheckedIndex opcode: Index/StoreIndex remain; proofs are counters.
    assert!(
        count_opcodes_in(&bc, start, end, Instruction::Index) >= 1,
        "nsieve should keep checked Index"
    );
    assert!(
        count_opcodes_in(&bc, start, end, Instruction::StoreIndex) >= 1,
        "nsieve should keep checked StoreIndex"
    );
    let stats = compiler::last_bounds_stats();
    assert!(
        stats.proven_index >= 1,
        "nsieve p-loop Index after fill-to-n should be proven; stats={stats:?}"
    );
    // Inner k = p+p / k += p is not a unit +1 counted form — stay checked.
    assert!(
        stats.checked_store_index >= 1,
        "nsieve StoreIndex on stride induction should stay checked; stats={stats:?}"
    );
}


#[test]
fn perf_bool_guard_inverts_into_jmpt() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/bool_guard.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "count_until", bc.len());
    // `if stop { break }` loads a bool: nothing to fuse into *Jmpf, so the
    // JMPF-over-JMP pair collapses to a single JMPT.
    assert_eq!(
        count_opcodes_in(&bc, start, end, Instruction::JMPT),
        1,
        "bool guard should invert to JMPT"
    );
    assert_eq!(
        count_opcodes_in(&bc, start, end, Instruction::JMPF),
        0,
        "no bare JMPF should remain in the guard"
    );
}

#[test]
fn perf_mandelbrot_keeps_fused_jmpf_guards() {
    // Guard inversion must refuse fusable conditions: there is no *Jmpt
    // superinstruction, so inverting `CmpJmpf` would add a dispatch.
    let (bc, _, _, _, pipeline) = compile("examples/perf/mandelbrot.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "mandelbrot", bc.len());
    assert_eq!(
        count_opcodes_in(&bc, start, end, Instruction::JMPT),
        0,
        "mandelbrot's compare guards must stay fused as *Jmpf"
    );
    assert!(
        count_opcodes_in(&bc, start, end, Instruction::BinSlotSlotConstJmpf) >= 1,
        "escape test should fuse to BinSlotSlotConstJmpf"
    );
    assert_eq!(
        count_opcodes_in(&bc, start, end, Instruction::CmpJmpf),
        0,
        "escape CmpJmpf should be absorbed into BinSlotSlotConstJmpf"
    );
}

#[test]
fn perf_mandelbrot_squares_fuse_into_bin_slot_slot() {
    // `zr * zr` / `zi * zi`: GVN's Dup is re-expanded so both operands fuse.
    let (bc, _, _, _, pipeline) = compile("examples/perf/mandelbrot.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "mandelbrot", bc.len());
    assert_eq!(
        count_opcodes_in(&bc, start, end, Instruction::DUPLICATE),
        0,
        "no DUPLICATE should survive in the float inner loop"
    );
    // Either fused form is fine: BinSlotSlot, or BinSlotSlotStore when the
    // result is stored straight into a slot.
    let self_mulf = bc[start..end]
        .iter()
        .filter(|b| match *b.bytecode() {
            Instruction::BinSlotSlot => {
                let (op, a, c) = b.bin_slot_slot_parts();
                op == Instruction::MULF as u8 && a == c
            }
            Instruction::BinSlotSlotStore => {
                let (op, a, c, _) = b.bin_slot_slot_store_parts();
                op == Instruction::MULF as u8 && a == c
            }
            _ => false,
        })
        .count();
    assert!(
        self_mulf >= 2,
        "zr*zr and zi*zi should each fuse to one self-MULF op, got {self_mulf}"
    );
}

#[test]
fn perf_mandelbrot_fuses_source_order_float_chain() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/mandelbrot.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "mandelbrot", bc.len());
    let chains = count_opcodes_in(&bc, start, end, Instruction::FloatChainStore);
    assert!(
        chains >= 2,
        "Mandelbrot should fuse both tr and zi source-ordered float stores, got {chains}"
    );
    // zi = 2.0 * (zr * zi) + ci must not remain as CONST + BinSlotSlot + MULF + …
    let mut unfused_zi = false;
    let slice = &bc[start..end];
    for i in 0..slice.len().saturating_sub(5) {
        if *slice[i].bytecode() != Instruction::CONST {
            continue;
        }
        if slice[i].operand_u32() & common::Byte::POOL_FLAG == 0 {
            continue;
        }
        if *slice[i + 1].bytecode() != Instruction::BinSlotSlot {
            continue;
        }
        let (op, _, _) = slice[i + 1].bin_slot_slot_parts();
        if op != Instruction::MULF as u8 {
            continue;
        }
        if *slice[i + 2].bytecode() == Instruction::MULF
            && *slice[i + 3].bytecode() == Instruction::LOAD
            && *slice[i + 4].bytecode() == Instruction::ADDF
            && matches!(
                *slice[i + 5].bytecode(),
                Instruction::STORE | Instruction::StorePop
            )
        {
            unfused_zi = true;
            break;
        }
    }
    assert!(
        !unfused_zi,
        "zi update should fuse to FloatChainStore, not CONST;BinSlotSlot;MULF;LOAD;ADDF;STORE"
    );
}

#[test]
fn perf_mandelbrot_hoists_invariant_ci_out_of_x_loop() {
    // `ci = 2.0 * (y as float)/(size as float) - 1.0` is invariant in the
    // x-loop. After LICM it must not recompute via BinSlotSlot DIVF + SUBF
    // between the x-loop header and the iter-loop header.
    // Use from_file so module resolution matches `coil dissect` / production.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let path = root.join("examples/perf/mandelbrot.hy");
    let mut pipeline = compiler::Pipeline::new();
    let (bc, _) = pipeline
        .compile_src_from_file(path.to_str().unwrap())
        .expect("compile mandelbrot from file");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "mandelbrot", bc.len());
    let body = &bc[start..end];

    // Nested counted loops: y-header, x-header, iter-header (BinSlotSlotJmpf).
    let headers: Vec<usize> = body
        .iter()
        .enumerate()
        .filter_map(|(i, b)| (*b.bytecode() == Instruction::BinSlotSlotJmpf).then_some(i))
        .collect();
    assert!(
        headers.len() >= 3,
        "expected y/x/iter loop headers, got {}",
        headers.len()
    );
    let x_header = headers[1];
    let iter_header = headers[2];
    let x_prefix = &body[x_header..iter_header];

    let bin_slot_divf = x_prefix
        .iter()
        .filter(|b| {
            *b.bytecode() == Instruction::BinSlotSlot
                && b.bin_slot_slot_parts().0 == Instruction::DIVF as u8
        })
        .count();
    let subf = x_prefix
        .iter()
        .filter(|b| *b.bytecode() == Instruction::SUBF)
        .count();
    assert_eq!(
        bin_slot_divf, 0,
        "ci's BinSlotSlot DIVF must leave the x-loop body (before iter header)"
    );
    // `cr` still ends with SUBF in the x-loop; ci's trailing SUBF must not
    // add a second one in the x-prefix (only cr's scale-subtract remains).
    assert_eq!(
        subf, 1,
        "x-loop prefix should keep only cr's SUBF; ci SUBF must be hoisted (got {subf})"
    );
}

#[test]
fn perf_mandelbrot_slot_promote_drops_ci_temp_copy() {
    // LICM hoists `ci` into a temp; slot promotion must rewrite uses to that
    // temp and elide the per-pixel `LOAD temp; STORE ci` copy.
    let (bc, pool, _, _, pipeline) = compile("examples/perf/mandelbrot.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "mandelbrot", bc.len());
    let body = &bc[start..end];

    let mut copy_temp_to_local = false;
    for i in 0..body.len().saturating_sub(1) {
        if *body[i].bytecode() != Instruction::LOAD {
            continue;
        }
        if body[i].load_store_single_slot() != Some(15) {
            continue;
        }
        if matches!(
            *body[i + 1].bytecode(),
            Instruction::STORE | Instruction::StorePop
        ) && body[i + 1].load_store_single_slot() == Some(6)
        {
            copy_temp_to_local = true;
            break;
        }
    }
    assert!(
        !copy_temp_to_local,
        "slot promote should drop LOAD 15; STORE 6 after rewriting ci uses"
    );

    // zi FloatChainStore should read the hoisted temp (15), not local slot 6.
    let mut zi_uses_temp = false;
    for b in body {
        if *b.bytecode() != Instruction::FloatChainStore {
            continue;
        }
        let op = b.operand_u32();
        let dest = (op >> 16) as u8;
        let di = (op & 0xffff) as usize;
        if dest != 8 || di >= pool.len() {
            continue;
        }
        let d = pool[di];
        let rhs2 = ((d >> 48) & 0xff) as u8;
        if rhs2 == 15 {
            zi_uses_temp = true;
        }
    }
    assert!(
        zi_uses_temp,
        "zi FloatChainStore should consume hoisted ci temp slot 15"
    );

    let loads = count_opcodes_in(&bc, start, end, Instruction::LOAD);
    let stores = count_opcodes_in(&bc, start, end, Instruction::STORE)
        + count_opcodes_in(&bc, start, end, Instruction::StorePop);
    assert!(
        loads <= 6,
        "mandelbrot LOAD count regressed after slot promote: {loads}"
    );
    assert!(
        stores <= 10,
        "mandelbrot STORE count regressed after slot promote: {stores}"
    );
}
