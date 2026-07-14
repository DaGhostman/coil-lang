use std::io::Read;

use common::{ARCHIVE_VERSION, ArchivedArchivedProgram, Byte};
use compiler::Pipeline;
use machine::Machine;
use rkyv::rancor::Error;

fn main() {
    let argc = std::env::args().collect::<Vec<_>>();
    let filename = argc[1].clone();

    if !std::fs::exists("out.c0s").expect("Unable to determine if file exists") {
        let pipeline = Pipeline::new();

        pipeline.compile(filename, "out.c0s".to_string());
    }

    let mut f = std::fs::File::open("out.c0s").expect("Unable to find file");
    let mut buffer = Vec::with_capacity(1024);
    f.read_to_end(&mut buffer).expect("Unable to read file");

    let archived = rkyv::access::<ArchivedArchivedProgram, Error>(&buffer)
        .expect("Unable to decode rkyv binary");

    if archived.version != ARCHIVE_VERSION {
        eprintln!(
            "Bytecode archive version {} does not match compiler version {}. Please recompile from source.",
            archived.version, ARCHIVE_VERSION
        );
        std::process::exit(1);
    }

    let bytecode = rkyv::deserialize::<Vec<Byte>, Error>(&archived.bytecode)
        .expect("Unable to deserialize bytecode");
    let constants = rkyv::deserialize::<Vec<u64>, Error>(&archived.constants)
        .expect("Unable to deserialize constant pool");

    Machine::<64>::default().run_raw(&bytecode, &constants);
}
