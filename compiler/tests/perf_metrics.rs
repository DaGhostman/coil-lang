//! Bytecode-shape and dispatch-count regression guards for the VM perf pass.

use compiler::Pipeline;
use common::{Byte, Instruction};
use machine::{dispatch_count, reset_dispatch_count, Machine};

fn compile(path: &str) -> (Vec<Byte>, Vec<u64>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let src = std::fs::read_to_string(root.join(path))
        .unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut pipeline = Pipeline::new();
    pipeline
        .compile_src(&src)
        .unwrap_or_else(|_| panic!("compile failed: {path}"))
}

fn count_opcodes(bytecode: &[Byte], op: Instruction) -> usize {
    bytecode.iter().filter(|b| *b.bytecode() == op).count()
}

fn run_dispatch(bytecode: Vec<Byte>, constants: Vec<u64>) -> u64 {
    reset_dispatch_count();
    let mut machine = Machine::<256>::default();
    machine.run_raw(&bytecode, &constants);
    dispatch_count()
}

#[test]
fn perf_numeric_uses_bin_slot_imm_jmpf_for_loop() {
    let (bc, _) = compile("examples/perf/numeric.0s");
    assert!(
        count_opcodes(&bc, Instruction::BinSlotImmJmpf) >= 1,
        "numeric loop should fuse compare+branch"
    );
}

#[test]
fn perf_operators_loop_uses_log_not_jmpf() {
    let (bc, _) = compile("examples/perf/operators_loop.0s");
    assert!(
        count_opcodes(&bc, Instruction::LogNotJmpf) >= 1,
        "operators loop should fuse LogNot; JMPF"
    );
}

#[test]
fn perf_numeric_dispatch_regression() {
    let (bc, pool) = compile("examples/perf/numeric.0s");
    let dispatches = run_dispatch(bc, pool);
    // Release of VM perf pass: loop compare+branch fused; expect well under 80k.
    assert!(
        dispatches < 80_000,
        "numeric dispatch count regressed: {dispatches}"
    );
}

#[test]
fn perf_match_sum_emits_jump_if_match() {
    let (bc, _) = compile("examples/perf/match_sum.0s");
    assert!(
        count_opcodes(&bc, Instruction::JumpIfMatch) >= 1,
        "match_sum should emit match dispatch"
    );
}

#[test]
fn perf_fib_dispatch_regression() {
    let (bc, pool) = compile("examples/fib_bench.0s");
    let dispatches = run_dispatch(bc, pool);
    assert!(
        dispatches < 18_000_000,
        "fib(32) dispatch count regressed: {dispatches}"
    );
}
