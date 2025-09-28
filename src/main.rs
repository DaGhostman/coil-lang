// use common::{Byte, Instruction, Value};
use compiler::{Pipeline};
use machine::{Machine};


fn main() {
    let argc = std::env::args().collect::<Vec<_>>();
    let filename = argc[1].clone();

    let pipeline = Pipeline::new();

    if let Ok(bytecode) = pipeline.run(filename) {
        Machine::<128>::default().run(&bytecode);
    }
}
