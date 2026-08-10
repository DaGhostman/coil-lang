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

/// Residual `LOAD`/`STORE` shape in a PC range.
///
/// `*_ops` counts instruction words, `*_slots` the slots they move (a packed
/// word carries up to 3). `packed_*_ops` are the words with `n > 1`.
#[derive(Debug, Default, PartialEq, Eq)]
struct LoadStoreShape {
    load_ops: usize,
    load_slots: usize,
    packed_load_ops: usize,
    store_ops: usize,
    store_slots: usize,
    packed_store_ops: usize,
}

fn load_store_shape(bytecode: &[Byte], start: usize, end: usize) -> LoadStoreShape {
    let mut shape = LoadStoreShape::default();
    for b in &bytecode[start..end] {
        let n = b.load_store_count();
        match *b.bytecode() {
            Instruction::LOAD => {
                shape.load_ops += 1;
                shape.load_slots += n;
                shape.packed_load_ops += usize::from(n > 1);
            }
            Instruction::STORE => {
                shape.store_ops += 1;
                shape.store_slots += n;
                shape.packed_store_ops += usize::from(n > 1);
            }
            _ => {}
        }
    }
    shape
}

/// Fused `BinSlot*` words in a PC range — the slot-addressed shapes that
/// already bypass a stack round-trip.
fn count_bin_slot_family_in(bytecode: &[Byte], start: usize, end: usize) -> usize {
    bytecode[start..end]
        .iter()
        .filter(|b| {
            matches!(
                *b.bytecode(),
                Instruction::BinSlotImm
                    | Instruction::BinSlotSlot
                    | Instruction::BinSlotImmJmpf
                    | Instruction::BinSlotSlotJmpf
                    | Instruction::BinSlotImmStore
                    | Instruction::BinSlotSlotStore
            )
        })
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
        count_opcodes_in(&bc, start, end, Instruction::CmpJmpf) >= 1,
        "escape test should stay a fused CmpJmpf"
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

// ---------------------------------------------------------------------------
// AOT harvest — Phase 0 inventory
//
// Soft ceilings on the bytecode shapes that later phases are supposed to move.
// Each ceiling is the count measured at Phase 0, so a regression trips the
// assert and a real win makes the ceiling stale (tighten it in that phase).
// Every test also prints its measured shape under `--nocapture`.
//
// Counter ownership:
//   P1 — residual LOAD / STORE (op + slot counts, packed vs single) and the
//        BinSlot* family that already avoids the stack round-trip. Landed:
//        `il::opt::slot_promote`. Still open, and both out of its reach: the
//        loop-carried cursor drift that leaves inner-loop stores non-redundant
//        (`mandelbrot`), and `Bin(slot, TOS)` operand shapes.
//   P2 — Index / StoreIndex in array-hot fns.
//   P3 — MakeEnum / MakeTuple / MakeArray allocation sites.
//   P4 — CALL / TailCall density in recursion-hot fns.
// ---------------------------------------------------------------------------

/// P1 + P4 baseline: `mandelbrot`'s float loops keep 8 LOADs / 13 STOREs, all
/// single-slot, against 13 already-fused `BinSlot*` words and zero calls.
#[test]
fn aot_p1_mandelbrot_residual_load_store_inventory() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/mandelbrot.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "mandelbrot", bc.len());
    let shape = load_store_shape(&bc, start, end);
    let fused = count_bin_slot_family_in(&bc, start, end);
    eprintln!("[P1] mandelbrot::mandelbrot {shape:?} bin_slot_family={fused}");

    assert!(
        shape.load_ops <= 8,
        "mandelbrot residual LOAD regressed: {shape:?}"
    );
    assert!(
        shape.store_ops <= 13,
        "mandelbrot residual STORE regressed: {shape:?}"
    );
    // Nothing in the float loops packs today: every LOAD/STORE moves one slot.
    assert_eq!(shape.load_slots, shape.load_ops, "{shape:?}");
    assert_eq!(shape.store_slots, shape.store_ops, "{shape:?}");
    assert_eq!(shape.packed_load_ops, 0, "{shape:?}");
    assert_eq!(shape.packed_store_ops, 0, "{shape:?}");
    assert!(
        fused >= 13,
        "mandelbrot lost fused BinSlot* coverage: {fused}"
    );
    assert_eq!(
        count_opcodes_in(&bc, start, end, Instruction::CALL),
        0,
        "mandelbrot must stay call-free"
    );
}

/// P1 + P4: `tak` is call-dominated — 3 `CALL` + 1 `TailCall`. Slot promotion
/// took the three argument temps out of the frame, so the reload run in front of
/// the `TailCall` and all three spill STOREs are gone (Phase 0: 4 packed LOADs
/// over 9 slots, 3 single-slot STOREs).
#[test]
fn aot_p1_p4_tak_residual_load_store_and_call_density() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/tak.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "tak", bc.len());
    let shape = load_store_shape(&bc, start, end);
    let calls = count_opcodes_in(&bc, start, end, Instruction::CALL);
    let tail_calls = count_opcodes_in(&bc, start, end, Instruction::TailCall);
    let fused = count_bin_slot_family_in(&bc, start, end);
    eprintln!(
        "[P1/P4] tak::tak {shape:?} call={calls} tail_call={tail_calls} bin_slot_family={fused} words={}",
        end - start
    );

    assert!(
        shape.load_ops <= 3,
        "tak residual LOAD regressed: {shape:?}"
    );
    assert_eq!(
        shape.store_ops, 0,
        "tak argument temps must stay promoted out of the frame: {shape:?}"
    );
    // Argument setup for the three recursive calls is fully packed.
    assert_eq!(shape.packed_load_ops, shape.load_ops, "{shape:?}");
    assert!(
        shape.load_slots >= 6,
        "tak should still pack the 6 forwarded argument loads: {shape:?}"
    );
    assert_eq!(calls, 3, "tak call density changed");
    assert!(
        tail_calls >= 1,
        "tak outer self-call should stay a TailCall"
    );
    assert!(fused >= 4, "tak lost fused BinSlot* coverage: {fused}");
}

/// P2 baseline: `nsieve`'s hot loops keep exactly one `Index` and one
/// `StoreIndex` alongside 5 LOADs / 5 STOREs.
#[test]
fn aot_p2_nsieve_index_shape_inventory() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/nsieve.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let (start, end) = fn_pc_range(&syms, "nsieve", bc.len());
    let index = count_opcodes_in(&bc, start, end, Instruction::Index);
    let store_index = count_opcodes_in(&bc, start, end, Instruction::StoreIndex);
    let shape = load_store_shape(&bc, start, end);
    eprintln!("[P2] nsieve::nsieve index={index} store_index={store_index} {shape:?}");

    // `flags[p]` read and `flags[k] = 0` write — one site each in source.
    assert_eq!(index, 1, "nsieve Index count changed");
    assert_eq!(store_index, 1, "nsieve StoreIndex count changed");
    assert!(
        shape.load_ops <= 5,
        "nsieve residual LOAD regressed: {shape:?}"
    );
    assert!(
        shape.store_ops <= 5,
        "nsieve residual STORE regressed: {shape:?}"
    );
    assert_eq!(shape.packed_store_ops, 0, "{shape:?}");
    // `flags.push(1)` is still an out-of-line Vec::push call.
    assert_eq!(
        count_opcodes_in(&bc, start, end, Instruction::CALL),
        1,
        "nsieve call count changed"
    );
}

/// P3 + P4 baseline: `bottom_up` allocates both `Tree` variants (2 `MakeEnum`)
/// per level and `item_check` unpacks without re-allocating.
#[test]
fn aot_p3_binary_trees_make_enum_inventory() {
    let (bc, _, _, _, pipeline) = compile("examples/perf/binary_trees.hy");
    let syms = pipeline.program_debug().fn_symbols;
    let mut total_enums = 0usize;
    let mut total_tuples = 0usize;
    let mut total_arrays = 0usize;
    let mut total_calls = 0usize;
    for name in ["bottom_up", "item_check", "main"] {
        let (start, end) = fn_pc_range(&syms, name, bc.len());
        let make_enum = count_opcodes_in(&bc, start, end, Instruction::MakeEnum);
        let make_tuple = count_opcodes_in(&bc, start, end, Instruction::MakeTuple);
        let make_array = count_opcodes_in(&bc, start, end, Instruction::MakeArray);
        let calls = count_opcodes_in(&bc, start, end, Instruction::CALL);
        eprintln!(
            "[P3/P4] binary_trees::{name} make_enum={make_enum} make_tuple={make_tuple} make_array={make_array} call={calls}"
        );
        total_enums += make_enum;
        total_tuples += make_tuple;
        total_arrays += make_array;
        total_calls += calls;
    }

    let (bottom_up_start, bottom_up_end) = fn_pc_range(&syms, "bottom_up", bc.len());
    assert_eq!(
        count_opcodes_in(&bc, bottom_up_start, bottom_up_end, Instruction::MakeEnum),
        2,
        "bottom_up should allocate exactly Leaf + Node"
    );
    let (check_start, check_end) = fn_pc_range(&syms, "item_check", bc.len());
    assert_eq!(
        count_opcodes_in(&bc, check_start, check_end, Instruction::MakeEnum),
        0,
        "item_check must not re-allocate while walking"
    );
    assert_eq!(
        count_opcodes_in(&bc, check_start, check_end, Instruction::Unpack),
        1,
        "item_check should keep one payload Unpack"
    );

    // User fns only: `format` needs 2 MakeTuple in main, no arrays anywhere.
    assert!(
        total_enums <= 2,
        "binary_trees user MakeEnum regressed: {total_enums}"
    );
    assert!(
        total_tuples <= 2,
        "binary_trees user MakeTuple regressed: {total_tuples}"
    );
    assert_eq!(total_arrays, 0, "binary_trees should not build arrays");
    assert!(
        total_calls <= 11,
        "binary_trees user CALL density regressed: {total_calls}"
    );
}
