use std::io::Read;

use common::{ARCHIVE_VERSION, ArchivedArchivedProgram, ArchivedProgram, Byte};
use compiler::Pipeline;
use machine::Machine;
use rkyv::rancor::Error;

fn compile_to_archive(filename: &str, output: &str) {
    let src =
        std::fs::read_to_string(filename).expect("Unable to read source file for compilation");
    let mut pipeline = Pipeline::new();
    let (bytecode, constants) = pipeline
        .compile_src(&src)
        .expect("Compilation failed (parse or type errors)");

    let program = ArchivedProgram {
        version: ARCHIVE_VERSION,
        constants,
        bytecode,
    };

    let bytes = rkyv::to_bytes::<Error>(&program).expect("Unable to serialize bytecode archive");
    std::fs::write(output, bytes.as_slice()).expect("Unable to write compiled output to file");
}

fn main() {
    let argc = std::env::args().collect::<Vec<_>>();
    let filename = argc[1].clone();

    if !std::fs::exists("out.c0s").expect("Unable to determine if file exists") {
        compile_to_archive(&filename, "out.c0s");
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

    let entry = std::path::Path::new(&filename);
    let pipeline = Pipeline::new();
    let mut machine = Machine::<256>::default();
    pipeline.wire_vm_ffi(&mut machine, Some(entry));
    machine.run_raw(&bytecode, &constants);
}
