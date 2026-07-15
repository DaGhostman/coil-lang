use common::{ArchivedByte as Byte, ArchivedInstruction as Instruction};
use criterion::{Criterion, criterion_group, criterion_main};
use machine::Machine;
use std::hint::black_box;

// fib body starts at offset 3 (right after the prologue: CONST, CALL, HALT).
const FIB_ENTRY: u32 = 3;

fn fib(n: u16) {
    let leq = Instruction::LEQ as u8;
    let sub = Instruction::SUB as u8;
    let add = Instruction::ADD as u8;
    let fib = [
        Byte::new(Instruction::CONST).with_const_inline(n as i32),
        Byte::new(Instruction::CALL).with_call_packed(1, FIB_ENTRY),
        Byte::new(Instruction::HALT),
        // 3: if !(n <= 2) jump to 6 (recurse); else fall through.
        Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(leq, 0, 2),
        Byte::new(Instruction::JMPF).with_operand_u32(6),
        // 5: base case → return 1.
        Byte::new(Instruction::ConstReturnImm).with_operand_u32(1),
        // 6: fib(n - 1)
        Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(sub, 0, 1),
        Byte::new(Instruction::CALL).with_call_packed(1, FIB_ENTRY),
        // 8: fib(n - 2)
        Byte::new(Instruction::LOAD).with_operand_u32(0),
        Byte::new(Instruction::BinSlotImm).with_bin_slot_imm(sub, 0, 2),
        Byte::new(Instruction::CALL).with_call_packed(1, FIB_ENTRY),
        // 11: return fib(n-1) + fib(n-2)
        Byte::new(Instruction::BinReturn).with_bin_return(add),
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
