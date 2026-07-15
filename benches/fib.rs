use common::{ArchivedByte as Byte, ArchivedInstruction as Instruction};
use criterion::{Criterion, criterion_group, criterion_main};
use machine::Machine;
use std::hint::black_box;

// fib body starts at offset 3 (right after the prologue: CONST, CALL, HALT).
const FIB_ENTRY: u32 = 3;

fn fib(n: u16) -> () {
    let fib = [
        Byte::new(Instruction::CONST).with_const_inline(n as i32),
        Byte::new(Instruction::CALL).with_call_packed(1, FIB_ENTRY),
        Byte::new(Instruction::HALT),
        Byte::new(Instruction::JmpfLeqSlotImm).with_jmpf_leq_slot_imm(0, 2, 8),
        Byte::new(Instruction::CONST).with_const_inline(1),
        Byte::new(Instruction::RETURN),
        Byte::new(Instruction::SubCallSlotImm).with_sub_call_slot_imm(0, 1, FIB_ENTRY as u16),
        Byte::new(Instruction::LOAD).with_operand_u32(0),
        Byte::new(Instruction::SubCallSlotImm).with_sub_call_slot_imm(0, 2, FIB_ENTRY as u16),
        Byte::new(Instruction::ADD),
        Byte::new(Instruction::RETURN),
    ];

    Machine::<512>::default().run(fib.as_slice())
}

fn fib_benchmark(c: &mut Criterion) {
    for n in 13..=32 {
        c.bench_function(&format!("fib #{}", n), |b| b.iter(|| fib(black_box(n))));
    }
}

criterion_group!(benches, fib_benchmark);
criterion_main!(benches);
