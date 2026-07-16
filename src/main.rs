use std::io::Read;
use std::path::Path;
use std::time::SystemTime;

use common::{ARCHIVE_VERSION, ArchivedArchivedProgram, ArchivedProgram, Byte};
use compiler::Pipeline;
use machine::Machine;
use rkyv::rancor::Error;

fn compile_to_archive(filename: &str, output: &str) {
    let mut pipeline = Pipeline::new();
    // Multi-file entry: discovers `use` / `mod` via zero.toml.
    let (bytecode, constants) = pipeline
        .compile_src_from_file(filename)
        .expect("Compilation failed (parse or type errors)");

    let program = ArchivedProgram {
        version: ARCHIVE_VERSION,
        constants,
        bytecode,
    };

    let bytes = rkyv::to_bytes::<Error>(&program).expect("Unable to serialize bytecode archive");
    std::fs::write(output, bytes.as_slice()).expect("Unable to write compiled output to file");
}

fn archive_mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn source_newer_than_archive(filename: &str, archive: &str) -> bool {
    match (archive_mtime(archive), archive_mtime(filename)) {
        (Some(arch), Some(src)) => src > arch,
        _ => false,
    }
}

fn try_load_archive(path: &str) -> Result<(Vec<Byte>, Vec<u64>), LoadErr> {
    let mut f = std::fs::File::open(path).map_err(|_| LoadErr::Missing)?;
    let mut buffer = Vec::with_capacity(1024);
    f.read_to_end(&mut buffer).map_err(|_| LoadErr::Corrupt)?;
    let archived =
        rkyv::access::<ArchivedArchivedProgram, Error>(&buffer).map_err(|_| LoadErr::Corrupt)?;
    let version = u32::from(archived.version);
    if version != ARCHIVE_VERSION {
        return Err(LoadErr::Version(version));
    }
    let bytecode = rkyv::deserialize::<Vec<Byte>, Error>(&archived.bytecode)
        .map_err(|_| LoadErr::Corrupt)?;
    let constants = rkyv::deserialize::<Vec<u64>, Error>(&archived.constants)
        .map_err(|_| LoadErr::Corrupt)?;
    Ok((bytecode, constants))
}

#[derive(Debug)]
enum LoadErr {
    Missing,
    Corrupt,
    Version(u32),
}

fn main() {
    let argc = std::env::args().collect::<Vec<_>>();
    let filename = argc[1].clone();
    const OUT: &str = "out.c0s";

    let cached = try_load_archive(OUT);
    let recompile = match &cached {
        Err(LoadErr::Missing) => true,
        Err(LoadErr::Corrupt) => true,
        Err(LoadErr::Version(v)) => {
            eprintln!(
                "Bytecode archive version {} does not match compiler version {}. Recompiling.",
                v, ARCHIVE_VERSION
            );
            true
        }
        Ok(_) => source_newer_than_archive(&filename, OUT),
    };

    if recompile {
        let _ = std::fs::remove_file(OUT);
        compile_to_archive(&filename, OUT);
    }

    let (bytecode, constants) = if recompile {
        try_load_archive(OUT).expect("Unable to load freshly compiled archive")
    } else {
        cached.expect("archive checked above")
    };

    let entry = Path::new(&filename);
    let pipeline = Pipeline::new();
    let mut machine = Machine::<256>::default();
    pipeline.wire_vm_ffi(&mut machine, Some(entry));
    machine.run_raw(&bytecode, &constants);
}
