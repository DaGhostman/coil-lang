//! Bytecode-shape and dispatch-count regression guards for the VM perf pass.

use common::{Byte, Instruction};
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
    let (bc, _, _, _, _) = compile("examples/perf/operators_loop.hy");
    // `if (!(i & 1))` inverts so the fused header is BinSlotImmJmpf(BITAND),
    // not LogNotJmpf.
    assert_eq!(
        count_opcodes(&bc, Instruction::LogNotJmpf),
        0,
        "operators loop should not emit LogNotJmpf after if(!c) invert"
    );
    assert!(
        bc.iter().any(|b| {
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
fn perf_fib_dispatch_regression() {
    let (bc, pool, strings, statics, pipeline) = compile("examples/fib_bench.hy");
    let dispatches = run_dispatch(bc, pool, strings, statics, &pipeline);
    // fib(10) is ~445 dispatches with current fusion; keep a generous ceiling.
    // HostInvoke write_all path adds a few more than bare PRINT.
    assert!(
        dispatches < 2_000,
        "fib(10) dispatch count regressed: {dispatches}"
    );
}

#[test]
fn perf_field_hot_reuses_repeated_string_keys() {
    let (bc, _, _, _, _) = compile("examples/perf/field_hot.hy");
    // Point::twice_x / hot loop reuses "x"/"y" — STRING count stays small vs
    // naive per-access emit (200k iters × several fields would explode).
    // Default Show/String (`typeof` FQN) add a couple of STRINGs beyond field keys.
    let strings = count_opcodes(&bc, Instruction::STRING);
    assert!(
        strings <= 12,
        "field_hot should materialize field-name STRINGs once per key, got {strings}"
    );
    assert!(
        count_opcodes(&bc, Instruction::GetField) >= 1,
        "field_hot should emit GetField"
    );
}

#[test]
fn perf_for_in_array_uses_single_array_len() {
    let (bc, _, _, _, _) = compile("examples/for_in_array.hy");
    // Loop hoist emits one ArrayLen; builtin `Length__string__len` adds another.
    let n = count_opcodes(&bc, Instruction::ArrayLen);
    assert!(
        (1..=2).contains(&n),
        "for_in_array should hoist ArrayLen out of the loop (got {n})"
    );
}
