use common::{ArchivedByte as Byte, ArchivedInstruction as Instruction};
use criterion::{Criterion, criterion_group, criterion_main};
use machine::Machine;
use std::hint::black_box;

// fib body starts at offset 3 (after prologue: CONST, CALL, HALT).
const FIB_ENTRY: u32 = 3;

fn fib(n: u16) {
    let leq = Instruction::LEQ as u8;
    let sub = Instruction::SUB as u8;
    let add = Instruction::ADD as u8;
    // Pool: BinSlotImmJmpf packs imm (low) + false-target (high).
    let pool = [((5u64) << 32) | (2u16 as u64)];
    let fib = [
        Byte::new(Instruction::CONST).with_const_inline(n as i32),
        Byte::new(Instruction::CALL).with_call_packed(1, FIB_ENTRY),
        Byte::new(Instruction::HALT),
        // if !(n <= 2) jump to 5; else fall through to return 1.
        Byte::new(Instruction::BinSlotImmJmpf).with_bin_slot_imm_jmpf(leq, 0, 0),
        Byte::new(Instruction::ConstReturnImm).with_operand_u32(1),
        // fib(n - 1)
        Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(sub, 0, 1),
        Byte::new(Instruction::CALL).with_call_packed(1, FIB_ENTRY),
        // fib(n - 2)
        Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(sub, 0, 2),
        Byte::new(Instruction::CALL).with_call_packed(1, FIB_ENTRY),
        Byte::new(Instruction::BinReturn).with_bin_return(add),
    ];

    Machine::<512>::default().run_with_pool(fib.as_slice(), &pool, 0)
}

fn fib_benchmark(c: &mut Criterion) {
    for n in 13..=32 {
        c.bench_function(&format!("fib #{}", n), |b| b.iter(|| fib(black_box(n))));
    }
}

criterion_group!(benches, fib_benchmark);
criterion_main!(benches);
