use std::io::Read;

use common::ArchivedByte;
use compiler::Pipeline;
use machine::Machine;
use rkyv::{rancor::Error, vec::ArchivedVec};

fn main() {
    let argc = std::env::args().collect::<Vec<_>>();
    let filename = argc[1].clone();

    // if !std::fs::exists("out.c0s").expect("Unable to determine if file exists") {
    let pipeline = Pipeline::new();

    pipeline.compile(filename, "out.c0s".to_string());
    // }

    let mut f = std::fs::File::open("out.c0s").expect("Unable to find file");
    let mut buffer = Vec::with_capacity(1024);
    f.read_to_end(&mut buffer).expect("Unable to read file");

    let bytecode = rkyv::access::<ArchivedVec<ArchivedByte>, Error>(&buffer)
        .expect("Unable to decode rkyv binary");

    Machine::<64>::default().run(bytecode.as_slice());
}
