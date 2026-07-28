use std::{
    borrow::Borrow,
    collections::VecDeque,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use common::{
    ARCHIVE_VERSION, ArchivedArchivedProgram, ArchivedProgram, Byte, Instruction, ProgramDebug,
    Value,
};
use machine::{FfiError, FfiSignature, FfiType, Heap, HostClosureFn, NativeFn};
use parser::{Pratt, SimpleSpan, ast::Expression};
use reporting::{
    Diagnostic, DiagnosticSink, ErrorCode, Message, ReportConfig, SourceId, SourceMap,
    create_sink,
};
use rkyv::rancor::Error;

use crate::Compiler;
use crate::manifest::Manifest;
use crate::typechecking::IoBuiltin;
use crate::typechecking::ThreadBuiltin;

/// A queued file to compile, along with the path it was
/// discovered under. The pipeline processes queued files
/// in BFS order from the entry point.
#[derive(Debug)]
struct WorkItem {
    /// Absolute path to the file on disk.
    file: PathBuf,
    /// Module namespace, derived from the file's path
    /// relative to one of the manifest's search roots.
    /// `None` means the file is outside any search root
    /// (we still compile it, but its namespace is the
    /// bare file stem).
    namespace: Option<String>,
}

pub struct Pipeline {
    failed: bool,
    project_root: PathBuf,
    manifest: Manifest,
    bytecode: Vec<Byte>,
    /// Set of files already visited (used to short-circuit
    /// diamond dependencies in the worklist).
    ///
    /// A `Vec<PathBuf>` rather than a `HashSet` because
    /// typical projects have <100 source files and a
    /// linear scan is faster than hashing for that size.
    /// Each entry is checked exactly once per `enqueue_file`
    /// call, and the per-file `PathBuf` allocation dominates
    /// the linear scan cost.
    processed: Vec<PathBuf>,
    /// FIFO queue of files to process. Drained front-to-back.
    worklist: VecDeque<WorkItem>,
    /// Native functions registered by the host. The
    /// pipeline tracks these so it can register them
    /// with the typechecker when a native call is
    /// typechecked.
    natives: Vec<NativeDecl>,
    /// Host Rust closures registered via [`Self::register_host_native`].
    host_natives: Vec<std::sync::Arc<dyn NativeFn>>,
    /// The entry file (the file passed to `compile`).
    /// This file is special: it's the program root and
    /// lives in the top-level namespace (no prefix),
    /// regardless of its path on disk. Every other
    /// file gets its path-derived namespace.
    entry_file: Option<PathBuf>,
    /// Parsed-source cache: avoids re-reading files between discovery and compile.
    source_interner: common::Interner<PathBuf>,
    source_cache: Vec<Option<String>>,
    /// When true, harness tests are compiled into the program (see `--include-tests`).
    include_tests: bool,
    compiler: Compiler,
    /// Owned diagnostic sink (pretty / SARIF / LSP).
    sink: Box<dyn DiagnosticSink>,
    /// How many compiler messages have already been emitted to [`Self::sink`].
    messages_emitted: usize,
}

/// Native function declaration registered by the host.
#[derive(Debug, Clone)]
pub struct NativeDecl {
    pub name: String,
    pub namespace: String,
    pub sig: FfiSignature,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    /// Register a host native with an explicit [`FfiSignature`]
    /// and Rust closure. The signature is forwarded to the HM
    /// typechecker; the closure is stored for
    /// [`Self::wire_host_natives`].
    pub fn register_host_native<F>(&mut self, sig: FfiSignature, func: F) -> usize
    where
        F: Fn(&mut Heap, &[common::Value]) -> Result<Option<common::Value>, FfiError>
            + Send
            + Sync
            + 'static,
    {
        let params: Vec<crate::typechecking::ty::Ty> =
            sig.args.iter().copied().map(ffi_type_to_ty).collect();
        let ret = ffi_type_to_ty(sig.ret);
        self.compiler.register(&sig.name, &params, &ret);
        let id = self.host_natives.len();
        self.host_natives
            .push(std::sync::Arc::new(HostClosureFn::new(sig, func)));
        id
    }

    /// Register a native function's type signature (metadata
    /// only — no VM closure). Embedders that supply their own
    /// closures should prefer [`Self::register_host_native`].
    pub fn register_native_function(&mut self, name: String, namespace: String, sig: FfiSignature) {
        let params: Vec<crate::typechecking::ty::Ty> =
            sig.args.iter().copied().map(ffi_type_to_ty).collect();
        let ret = ffi_type_to_ty(sig.ret);
        self.compiler.register(&name, &params, &ret);
        self.natives.push(NativeDecl {
            name,
            namespace,
            sig,
        });
    }

    /// Wire host natives registered via [`Self::register_host_native`]
    /// into the VM. Call before `Machine::run_raw`.
    pub fn wire_host_natives<const N: usize>(&self, machine: &mut machine::Machine<N>) {
        for native in &self.host_natives {
            machine.register_native(std::sync::Arc::clone(native));
        }
    }

    /// Register virtual `io` host natives (`stdin`, `open`, `read`, …).
    ///
    /// Names are bound for [`Instruction::HostInvoke`] only — HM types come
    /// from `use io::*` / [`Checker::io_fn_scheme`], not from the FFI env.
    fn register_io_natives(&mut self) {
        use machine::io::{
            as_result_int, as_result_option_int, as_result_unit, as_result_value, from_bytes,
            stream_close, stream_open, stream_read, stream_read_exact, stream_read_to_end,
            stream_stderr, stream_stdin, stream_stdout, stream_write, stream_write_all, tcp_accept,
            tcp_accept_wait, tcp_connect, tcp_listen, to_bytes, udp_bind, udp_connect,
            udp_local_port, udp_recv_from, udp_recv_from_wait, udp_send_to, value_as_string,
        };

        for kind in IoBuiltin::all() {
            // Host registry keys stay uniquely prefixed for TCP/UDP
            // (`tcp_connect` vs surface `connect` under `io::net::tcp`).
            let name = kind.native_name().to_string();
            let arity = match kind {
                IoBuiltin::Stdin | IoBuiltin::Stdout | IoBuiltin::Stderr => 0,
                IoBuiltin::Close
                | IoBuiltin::ReadToEnd
                | IoBuiltin::FromBytes
                | IoBuiltin::ToBytes
                | IoBuiltin::TcpAccept
                | IoBuiltin::TcpAcceptWait
                | IoBuiltin::UdpLocalPort => 1,
                IoBuiltin::Open
                | IoBuiltin::Read
                | IoBuiltin::Write
                | IoBuiltin::ReadExact
                | IoBuiltin::WriteAll
                | IoBuiltin::TcpConnect
                | IoBuiltin::TcpListen
                | IoBuiltin::UdpBind
                | IoBuiltin::UdpConnect
                | IoBuiltin::UdpRecvFrom
                | IoBuiltin::UdpRecvFromWait => 2,
                IoBuiltin::UdpSendTo => 4,
            };
            let args = vec![FfiType::Int; arity];
            let sig = FfiSignature::from_parts(name.clone(), args, FfiType::Int)
                .expect("io native arity/signature");
            let id = self.host_natives.len();
            self.compiler.register_native_id(&name, id);
            let kind = *kind;
            self.host_natives
                .push(std::sync::Arc::new(HostClosureFn::new(sig, move |heap, args| {
                    let v = match kind {
                        // Stdio handles are `() -> Stream` (not Result).
                        IoBuiltin::Stdin => stream_stdin(heap).unwrap_or_default(),
                        IoBuiltin::Stdout => stream_stdout(heap).unwrap_or_default(),
                        IoBuiltin::Stderr => stream_stderr(heap).unwrap_or_default(),
                        IoBuiltin::Open => {
                            let path = match value_as_string(heap, args[0]) {
                                Ok(s) => s,
                                Err(tag) => {
                                    return Ok(Some(as_result_value(heap, Err(tag))));
                                }
                            };
                            let mode = match value_as_string(heap, args[1]) {
                                Ok(s) => s,
                                Err(tag) => {
                                    return Ok(Some(as_result_value(heap, Err(tag))));
                                }
                            };
                            let r = stream_open(heap, &path, &mode);
                            as_result_value(heap, r)
                        }
                        IoBuiltin::Close => {
                            let r = stream_close(heap, args[0]);
                            as_result_unit(heap, r)
                        }
                        IoBuiltin::Read => {
                            let r = stream_read(heap, args[0], args[1]);
                            as_result_option_int(heap, r)
                        }
                        IoBuiltin::Write => {
                            let r = stream_write(heap, args[0], args[1]);
                            as_result_int(heap, r)
                        }
                        IoBuiltin::ReadExact => {
                            let r = stream_read_exact(heap, args[0], args[1]);
                            as_result_option_int(heap, r)
                        }
                        IoBuiltin::ReadToEnd => {
                            let r = stream_read_to_end(heap, args[0]);
                            as_result_value(heap, r)
                        }
                        IoBuiltin::WriteAll => {
                            let r = stream_write_all(heap, args[0], args[1]);
                            as_result_unit(heap, r)
                        }
                        IoBuiltin::FromBytes => {
                            let r = from_bytes(heap, args[0]);
                            as_result_value(heap, r)
                        }
                        IoBuiltin::ToBytes => to_bytes(heap, args[0]),
                        IoBuiltin::TcpConnect => {
                            let host = match value_as_string(heap, args[0]) {
                                Ok(s) => s,
                                Err(tag) => {
                                    return Ok(Some(as_result_value(heap, Err(tag))));
                                }
                            };
                            let r = tcp_connect(heap, &host, args[1].as_int());
                            as_result_value(heap, r)
                        }
                        IoBuiltin::TcpListen => {
                            let host = match value_as_string(heap, args[0]) {
                                Ok(s) => s,
                                Err(tag) => {
                                    return Ok(Some(as_result_value(heap, Err(tag))));
                                }
                            };
                            let r = tcp_listen(heap, &host, args[1].as_int());
                            as_result_value(heap, r)
                        }
                        IoBuiltin::TcpAccept => {
                            let r = tcp_accept(heap, args[0]);
                            as_result_value(heap, r)
                        }
                        IoBuiltin::TcpAcceptWait => {
                            let r = tcp_accept_wait(heap, args[0]);
                            as_result_value(heap, r)
                        }
                        IoBuiltin::UdpBind => {
                            let host = match value_as_string(heap, args[0]) {
                                Ok(s) => s,
                                Err(tag) => {
                                    return Ok(Some(as_result_value(heap, Err(tag))));
                                }
                            };
                            let r = udp_bind(heap, &host, args[1].as_int());
                            as_result_value(heap, r)
                        }
                        IoBuiltin::UdpConnect => {
                            let host = match value_as_string(heap, args[0]) {
                                Ok(s) => s,
                                Err(tag) => {
                                    return Ok(Some(as_result_value(heap, Err(tag))));
                                }
                            };
                            let r = udp_connect(heap, &host, args[1].as_int());
                            as_result_value(heap, r)
                        }
                        IoBuiltin::UdpSendTo => {
                            let host = match value_as_string(heap, args[2]) {
                                Ok(s) => s,
                                Err(tag) => {
                                    return Ok(Some(as_result_value(heap, Err(tag))));
                                }
                            };
                            let r = udp_send_to(heap, args[0], args[1], &host, args[3].as_int());
                            as_result_int(heap, r)
                        }
                        IoBuiltin::UdpRecvFrom => {
                            let r = udp_recv_from(heap, args[0], args[1]);
                            as_result_value(heap, r)
                        }
                        IoBuiltin::UdpRecvFromWait => {
                            let r = udp_recv_from_wait(heap, args[0], args[1]);
                            as_result_value(heap, r)
                        }
                        IoBuiltin::UdpLocalPort => {
                            let r = udp_local_port(heap, args[0]).map(Value::from);
                            as_result_value(heap, r)
                        }
                    };
                    Ok(Some(v))
                })));
        }
    }

    /// Register virtual `thread` host natives (`spawn`, `channel`, …).
    fn register_thread_natives(&mut self) {
        use machine::thread;

        for kind in ThreadBuiltin::all() {
            let name = kind.native_name().to_string();
            let arity = match kind {
                ThreadBuiltin::Channel => 0,
                ThreadBuiltin::Spawn => 1,
                ThreadBuiltin::Recv
                | ThreadBuiltin::TryRecv
                | ThreadBuiltin::Close
                | ThreadBuiltin::Mutex
                | ThreadBuiltin::Rwlock
                | ThreadBuiltin::Lock
                | ThreadBuiltin::TryLock
                | ThreadBuiltin::Unlock => 1,
                ThreadBuiltin::Join | ThreadBuiltin::Detach => 1,
                ThreadBuiltin::Send
                | ThreadBuiltin::TrySend
                | ThreadBuiltin::WithLock
                | ThreadBuiltin::WithRead
                | ThreadBuiltin::WithWrite
                | ThreadBuiltin::TryRead
                | ThreadBuiltin::TryWrite => 2,
            };
            let args = vec![FfiType::Int; arity];
            let sig = FfiSignature::from_parts(name.clone(), args, FfiType::Int)
                .expect("thread native arity/signature");
            let id = self.host_natives.len();
            self.compiler.register_native_id(&name, id);
            let kind = *kind;
            let closure = move |heap: &mut machine::Heap, args: &[common::Value]| {
                let v = match kind {
                    ThreadBuiltin::Spawn => thread::thread_spawn(heap, args),
                    ThreadBuiltin::Join => thread::thread_join(heap, args),
                    ThreadBuiltin::Detach => thread::thread_detach(heap, args),
                    ThreadBuiltin::Channel => thread::thread_channel(heap, args),
                    ThreadBuiltin::Send => thread::thread_send(heap, args),
                    ThreadBuiltin::Recv => thread::thread_recv(heap, args),
                    ThreadBuiltin::TrySend => thread::thread_try_send(heap, args),
                    ThreadBuiltin::TryRecv => thread::thread_try_recv(heap, args),
                    ThreadBuiltin::Close => thread::thread_close(heap, args),
                    ThreadBuiltin::Mutex => thread::thread_mutex(heap, args),
                    ThreadBuiltin::WithLock => thread::thread_with_lock(heap, args),
                    ThreadBuiltin::Lock => thread::thread_lock(heap, args),
                    ThreadBuiltin::TryLock => thread::thread_try_lock(heap, args),
                    ThreadBuiltin::Unlock => thread::thread_unlock(heap, args),
                    ThreadBuiltin::Rwlock => thread::thread_rwlock(heap, args),
                    ThreadBuiltin::WithRead => thread::thread_with_read(heap, args),
                    ThreadBuiltin::WithWrite => thread::thread_with_write(heap, args),
                    ThreadBuiltin::TryRead => thread::thread_try_read(heap, args),
                    ThreadBuiltin::TryWrite => thread::thread_try_write(heap, args),
                };
                Ok(Some(v))
            };
            let native = if kind == ThreadBuiltin::Spawn {
                std::sync::Arc::new(HostClosureFn::new_with_arity_range(
                    sig, 1, 2, closure,
                )) as std::sync::Arc<dyn NativeFn>
            } else {
                std::sync::Arc::new(HostClosureFn::new(sig, closure)) as std::sync::Arc<dyn NativeFn>
            };
            self.host_natives.push(native);
        }
    }

    fn register_fs_natives(&mut self) {
        use machine::fs::FS_HOST_FUNCTIONS;
        use machine::{FfiSignature, FfiType, HostClosureFn};

        for &(name, host) in FS_HOST_FUNCTIONS {
            let arity = match name {
                "fs_rename" | "fs_copy" | "fs_symlink" => 2,
                _ => 1,
            };
            let args = vec![FfiType::Int; arity];
            let sig = FfiSignature::from_parts(name.to_string(), args, FfiType::Int)
                .expect("fs native signature");
            let id = self.host_natives.len();
            self.compiler.register_native_id(name, id);
            self.host_natives
                .push(std::sync::Arc::new(HostClosureFn::new(sig, move |heap, args| {
                    Ok(Some(host(heap, args)))
                })));
        }
    }

    fn register_time_natives(&mut self) {
        use machine::time::TIME_HOST_FUNCTIONS;
        use machine::{FfiSignature, FfiType, HostClosureFn};

        for &(name, host) in TIME_HOST_FUNCTIONS {
            let arity = match name {
                "time_period" => 9,
                "time_add" | "time_sub" | "time_period_add" | "time_period_sub" | "time_format"
                | "time_parse" => 2,
                "time_sleep_ms" | "time_elapsed_nanos" | "time_elapsed_millis"
                | "time_date_from_period" | "time_date_from_epoch_period" => 1,
                _ => 0,
            };
            let args = vec![FfiType::Int; arity];
            let sig = FfiSignature::from_parts(name.to_string(), args, FfiType::Int)
                .expect("time native signature");
            let id = self.host_natives.len();
            self.compiler.register_native_id(name, id);
            self.host_natives
                .push(std::sync::Arc::new(HostClosureFn::new(sig, move |heap, args| {
                    Ok(Some(host(heap, args)))
                })));
        }
    }

    fn register_env_natives(&mut self) {
        use machine::env::ENV_HOST_FUNCTIONS;
        use machine::{FfiSignature, FfiType, HostClosureFn};

        for &(name, host) in ENV_HOST_FUNCTIONS {
            let arity = match name {
                "env_args" => 0,
                "env_var" | "env_cwd" | "env_remove_var" | "env_exit" | "env_set_cwd" => 1,
                "env_set_var" | "env_exec" => 2,
                _ => 1,
            };
            let args = vec![FfiType::Int; arity];
            let sig = FfiSignature::from_parts(name.to_string(), args, FfiType::Int)
                .expect("env native signature");
            let id = self.host_natives.len();
            self.compiler.register_native_id(name, id);
            self.host_natives
                .push(std::sync::Arc::new(HostClosureFn::new(sig, move |heap, args| {
                    Ok(Some(host(heap, args)))
                })));
        }
    }

    fn register_crypto_natives(&mut self) {
        use machine::CRYPTO_WIRING;
        use machine::{FfiSignature, FfiType, HostClosureFn};

        for &(name, arity, host) in CRYPTO_WIRING {
            let args = vec![FfiType::Int; arity];
            let sig = FfiSignature::from_parts(name.to_string(), args, FfiType::Int)
                .expect("crypto native signature");
            let id = self.host_natives.len();
            self.compiler.register_native_id(name, id);
            self.host_natives
                .push(std::sync::Arc::new(HostClosureFn::new(sig, move |heap, args| {
                    Ok(Some(host(heap, args)))
                })));
        }
    }

    fn register_regex_natives(&mut self) {
        use machine::REGEX_WIRING;
        use machine::{FfiSignature, FfiType, HostClosureFn};

        for &(name, arity, host) in REGEX_WIRING {
            let args = vec![FfiType::Int; arity];
            let sig = FfiSignature::from_parts(name.to_string(), args, FfiType::Int)
                .expect("regex native signature");
            let id = self.host_natives.len();
            self.compiler.register_native_id(name, id);
            self.host_natives
                .push(std::sync::Arc::new(HostClosureFn::new(sig, move |heap, args| {
                    Ok(Some(host(heap, args)))
                })));
        }
    }

    /// Install shared bytecode on `machine` for `thread::spawn` workers.
    pub fn wire_thread_program<const N: usize>(
        &self,
        machine: &mut machine::Machine<N>,
        bytecode: &[Byte],
        constants: &[u64],
    ) {
        use machine::thread::ThreadProgram;
        use std::sync::Arc;
        machine.set_thread_program(Arc::new(ThreadProgram {
            code: Arc::from(bytecode.to_vec()),
            constants: Arc::from(constants.to_vec()),
            static_slot_count: self.static_slot_count(),
            debug: self.program_debug(),
        }));
    }

    /// Bytecode entry offset for a registered function (for tests).
    pub fn function_offset(&self, name: &str) -> Option<usize> {
        self.compiler.function_offset(name)
    }

    /// Prelude `ord` / `char` / string-`Hash` host natives (auto-imported).
    fn register_prelude_char_ord_natives(&mut self) {
        use machine::char_ord::{prelude_char, prelude_hash_string, prelude_ord};
        use machine::{FfiSignature, FfiType, HostClosureFn};

        let ord_sig =
            FfiSignature::from_parts("ord".to_string(), vec![FfiType::Int], FfiType::Int)
                .expect("ord signature");
        let ord_id = self.host_natives.len();
        self.compiler.register_native_id("ord", ord_id);
        self.host_natives
            .push(std::sync::Arc::new(HostClosureFn::new(ord_sig, |heap, args| {
                Ok(Some(prelude_ord(heap, args)))
            })));

        let char_sig =
            FfiSignature::from_parts("char".to_string(), vec![FfiType::Int], FfiType::Int)
                .expect("char signature");
        let char_id = self.host_natives.len();
        self.compiler.register_native_id("char", char_id);
        self.host_natives
            .push(std::sync::Arc::new(HostClosureFn::new(char_sig, |heap, args| {
                Ok(Some(prelude_char(heap, args)))
            })));

        // Internal: `Hash__string__hash` thunk — not a userland free function.
        let hash_sig =
            FfiSignature::from_parts("hash_string".to_string(), vec![FfiType::String], FfiType::Int)
                .expect("hash_string signature");
        let hash_id = self.host_natives.len();
        self.compiler.register_native_id("hash_string", hash_id);
        self.host_natives
            .push(std::sync::Arc::new(HostClosureFn::new(hash_sig, |heap, args| {
                Ok(Some(prelude_hash_string(heap, args)))
            })));
    }

    /// Approach A packed LA kernels via existing `HostInvoke` (no new opcodes —
    /// keeps the `Instruction` enum identical to `main` for fib dispatch).
    fn register_packed_la_natives(&mut self) {
        use machine::{
            PACKED_DOT, PACKED_MATMUL, PACKED_MATRIX_NEG, PACKED_MATRIX_ZIP, packed_dot,
            packed_matmul, packed_matrix_neg, packed_matrix_zip,
        };

        let specs: &[(&str, usize, fn(&mut machine::Heap, &[common::Value]) -> common::Value)] = &[
            (PACKED_DOT, 3, packed_dot),
            (PACKED_MATMUL, 3, packed_matmul),
            (PACKED_MATRIX_ZIP, 3, packed_matrix_zip),
            (PACKED_MATRIX_NEG, 2, packed_matrix_neg),
        ];
        for &(name, arity, kernel) in specs {
            let args = vec![FfiType::Int; arity];
            let sig = FfiSignature::from_parts(name.to_string(), args, FfiType::Int)
                .expect("packed LA native signature");
            let id = self.host_natives.len();
            self.compiler.register_native_id(name, id);
            self.host_natives
                .push(std::sync::Arc::new(HostClosureFn::new(sig, move |heap, args| {
                    Ok(Some(kernel(heap, args)))
                })));
        }
    }

    /// Borrow the inner `Compiler` mutably. Used by the
    /// integration tests in `compiler/src/lib.rs::tests`
    /// and `compiler/tests/namespace.rs` that need to
    /// inspect the compiler's diagnostic messages
    /// directly.
    #[cfg(test)]
    pub fn compiler_mut(&mut self) -> &mut Compiler {
        &mut self.compiler
    }

    /// Borrow the compiler's accumulated diagnostic
    /// messages. Public so integration tests can read
    /// them (the `#[cfg(test)]`-only `compiler_mut` is
    /// only visible to in-crate tests).
    pub fn messages(&self) -> &[Message] {
        self.compiler.get_messages()
    }

    /// Project root (directory containing `coil.toml`, or cwd).
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Loaded project manifest (`[entry]`, `[module].roots`, …).
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Resolve `[entry].file` from the manifest to an absolute path.
    /// Returns `None` when the manifest has no entry point.
    pub fn manifest_entry_path(&self) -> Option<PathBuf> {
        self.manifest
            .entry
            .as_ref()
            .map(|rel| self.project_root.join(rel))
    }

    /// Wire FFI library resolution paths and C struct layouts into the VM.
    pub fn wire_vm_ffi<const N: usize>(
        &self,
        vm: &mut machine::Machine<N>,
        entry_path: Option<&std::path::Path>,
    ) {
        use machine::{CStructLayout, FfiType};
        let base_dir = entry_path
            .and_then(|p| p.parent())
            .map(std::path::PathBuf::from);
        let search: Vec<std::path::PathBuf> = self
            .manifest
            .ffi_search_paths
            .iter()
            .map(|p| self.project_root.join(p))
            .collect();
        vm.set_ffi_paths(base_dir, search);
        for def in self.compiler.c_structs() {
            let fields = def
                .fields
                .iter()
                .map(|(name, enc)| {
                    let (tag, aux) = if *enc <= common::tag::STRUCT {
                        (*enc, 0)
                    } else {
                        (*enc & 0xFFFF, *enc >> 16)
                    };
                    (name.clone(), FfiType::from_tag(tag, aux))
                })
                .collect();
            vm.register_struct_layout(CStructLayout {
                name: def.name.clone(),
                fields,
            });
        }
    }

    /// Walk up from `start` looking for a directory that contains
    /// `coil.toml`. Falls back to the process cwd when none is found.
    fn find_project_root(start: &Path) -> PathBuf {
        let mut dir = if start.is_file() {
            start
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            start.to_path_buf()
        };
        loop {
            if dir.join("coil.toml").is_file() {
                return dir;
            }
            if !dir.pop() {
                break;
            }
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    pub fn new() -> Self {
        Self::with_reporter(ReportConfig::default(), Box::new(std::io::stderr()))
    }

    /// Construct a pipeline with an explicit diagnostic sink config and writer.
    ///
    /// Used by the CLI (`--log-json` / `--log-lsp`) and by unit tests that
    /// capture rendered diagnostics into a buffer.
    pub fn with_reporter(config: ReportConfig, writer: Box<dyn Write + Send>) -> Self {
        let cwd = std::env::current_dir().expect("Unable to determine current working directory");
        // Prefer a `coil.toml` found by walking up from cwd; otherwise
        // use cwd with the default manifest (`src/` only).
        let project_root = Self::find_project_root(&cwd);
        let sink = create_sink(&config, SourceMap::new(), writer);

        // The prologue is `[CALL, JMP, HALT]`. The pipeline
        // patches the JMP at offset 1 to point at `main`
        // (or `program_start_offset` if `extern` blocks ran
        // first). See `Self::prologue` for the layout.
        let bytecode = vec![
            Byte::new(Instruction::CALL),
            Byte::new(Instruction::JMP).with_operand_u32(u32::MAX),
            Byte::new(Instruction::HALT),
        ];

        let mut pipeline = Self {
            failed: false,
            project_root: project_root.clone(),
            manifest: Manifest::default(),
            bytecode,
            processed: Vec::new(),
            worklist: VecDeque::new(),
            natives: Vec::new(),
            host_natives: Vec::new(),
            entry_file: None,
            source_interner: common::Interner::default(),
            source_cache: Vec::new(),
            include_tests: false,
            compiler: Compiler::default(),
            sink,
            messages_emitted: 0,
        };
        match Manifest::load(&project_root) {
            Ok(m) => {
                pipeline.manifest = m.clone();
                machine::env::set_allow_exec(m.allow_exec);
            }
            Err(e) => pipeline.emit_manifest_load_error(&project_root, e),
        }
        pipeline.register_io_natives();
        pipeline.register_fs_natives();
        pipeline.register_time_natives();
        pipeline.register_env_natives();
        pipeline.register_crypto_natives();
        pipeline.register_regex_natives();
        pipeline.register_prelude_char_ord_natives();
        pipeline.register_thread_natives();
        pipeline.register_packed_la_natives();
        pipeline
    }

    /// Register `source` under `path` and emit a single producer [`Message`].
    ///
    /// Also records the message on the compiler so [`Self::messages`]
    /// includes discovery-time parse / module-not-found errors (not only
    /// typecheck diagnostics). Advances `messages_emitted` so a later
    /// [`Self::emit_new_messages`] does not re-forward the same text.
    fn emit_message(&mut self, path: &Path, source: &str, message: &Message) {
        self.compiler.push_message(message.clone());
        self.messages_emitted = self.compiler.get_messages().len();
        let file_id = self.sink.register_source(path, source);
        self.sink.emit(Diagnostic::from_message(message, file_id));
        if self.sink.had_errors() {
            self.failed = true;
        }
    }

    /// Emit compiler messages that have not yet been forwarded to the sink.
    fn emit_new_messages(&mut self, file_id: SourceId) {
        let all = self.compiler.get_messages();
        let pending: Vec<Message> = all[self.messages_emitted..].to_vec();
        self.messages_emitted = all.len();
        for msg in &pending {
            self.sink.emit(Diagnostic::from_message(msg, file_id));
        }
        if self.sink.had_errors() {
            self.failed = true;
        }
    }

    /// Emit a CLI / I/O style error with no source span.
    pub fn emit_spanless_error(&mut self, code: ErrorCode, message: impl Into<String>) {
        self.sink.emit(Diagnostic::error(message).with_code(code));
        self.failed = true;
    }

    /// Emit a warning with no source span (e.g. sink flush failure).
    pub fn emit_spanless_warning(&mut self, code: ErrorCode, message: impl Into<String>) {
        self.sink
            .emit(Diagnostic::warning(message.into()).with_code(code));
    }

    fn emit_manifest_load_error(&mut self, project_root: &Path, err: crate::manifest::ManifestError) {
        let path = project_root.join("coil.toml");
        match err {
            crate::manifest::ManifestError::Parse { line, message } => {
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    let range = Manifest::byte_range_for_line(&contents, line);
                    let msg = Message::error(
                        ErrorCode::IoError,
                        format!("`coil.toml` parse error at line {line}: {message}"),
                        range,
                    );
                    self.emit_message(&path, &contents, &msg);
                } else {
                    self.emit_spanless_error(
                        ErrorCode::IoError,
                        format!("`{}`: parse error at line {line}: {message}", path.display()),
                    );
                }
            }
            crate::manifest::ManifestError::Io(msg) => {
                self.emit_spanless_error(ErrorCode::IoError, msg);
            }
            crate::manifest::ManifestError::MissingSection(section) => {
                self.emit_spanless_error(
                    ErrorCode::IoError,
                    format!(
                        "`{}`: missing manifest section `[{section}]`",
                        path.display()
                    ),
                );
            }
            crate::manifest::ManifestError::MissingKey { section, key } => {
                self.emit_spanless_error(
                    ErrorCode::IoError,
                    format!(
                        "`{}`: missing manifest key `[{section}].{key}`",
                        path.display()
                    ),
                );
            }
        }
    }

    fn emit_module_not_found(
        &mut self,
        parent_file: &Path,
        parent_src: &str,
        range: std::ops::Range<usize>,
        detail: impl Into<String>,
    ) {
        let msg = Message::error(
            ErrorCode::IoError,
            format!("Module not found: {}", detail.into()),
            range,
        );
        self.emit_message(parent_file, parent_src, &msg);
        self.failed = true;
    }

    fn format_use_path(path: &[String], name: &str) -> String {
        if path.is_empty() {
            name.to_string()
        } else {
            format!("{}::{}", path.join("::"), name)
        }
    }

    /// Flush the diagnostic sink (required for SARIF / LSP buffered formats).
    pub fn finish_reporting(&mut self) -> std::io::Result<()> {
        self.sink.finish()
    }

    /// True if any error diagnostic was emitted or a hard pipeline failure
    /// was recorded (e.g. unreadable source file).
    pub fn had_errors(&self) -> bool {
        self.failed || self.sink.had_errors()
    }

    /// First pass: walk the AST and enqueue every
    /// referenced module file. We do this WITHOUT
    /// compiling (so the worklist is complete before
    /// we touch `self.compiler`). This avoids the
    /// `&mut self` recursion issue.
    ///
    /// `use foo::bar;` and `mod foo;` are both
    /// discovered. `use foo::bar::*;` (glob) is the
    /// same as `use foo::bar;` for discovery purposes
    /// — we just need to load `foo::bar` so the
    /// compiler can resolve the items.
    fn enqueue_uses(
        &mut self,
        parent_file: &Path,
        parent_src: &str,
        ast: &(SimpleSpan, Box<Expression<'_>>),
    ) {
        let use_range = ast.0.into_range();
        match ast.1.borrow() {
            Expression::Use { path, name, .. } => {
                // Compiler virtual modules (`prelude`, `ffi`, …) are not
                // `.hy` files — skip disk discovery for those paths.
                {
                    use crate::typechecking::VirtualModules;
                    let vm = VirtualModules::new();
                    if vm.resolves_use(path, name) {
                        return;
                    }
                }
                if name == "*" {
                    let segments = path.clone();
                    if let Some(last) = segments.last().cloned() {
                        let mut segments = segments;
                        segments.pop();
                        if let Some(file) =
                            self.manifest
                                .resolve_use(&self.project_root, &segments, &last)
                        {
                            self.enqueue_file(file);
                        } else {
                            self.emit_module_not_found(
                                parent_file,
                                parent_src,
                                use_range,
                                format!("`use {}::*`", Self::format_use_path(path, "*")),
                            );
                        }
                    } else if let Some(file) = self.manifest.resolve_mod(&self.project_root, "*") {
                        self.enqueue_file(file);
                    } else {
                        self.emit_module_not_found(
                            parent_file,
                            parent_src,
                            use_range,
                            "`use *`",
                        );
                    }
                } else if let Some(file) =
                    self.manifest.resolve_use(&self.project_root, path, name)
                {
                    self.enqueue_file(file);
                } else {
                    self.emit_module_not_found(
                        parent_file,
                        parent_src,
                        use_range,
                        format!("`use {}`", Self::format_use_path(path, name)),
                    );
                }
            }
            Expression::Module(name, _body) => {
                if let Some(file) = self.manifest.resolve_mod(&self.project_root, name) {
                    self.enqueue_file(file);
                } else {
                    self.emit_module_not_found(
                        parent_file,
                        parent_src,
                        use_range,
                        format!("`mod {name}`"),
                    );
                }
            }
            Expression::Program(children)
            | Expression::Block(children)
            | Expression::Fragment(children) => {
                for child in children.iter() {
                    self.enqueue_uses(parent_file, parent_src, child);
                }
            }
            _ => (),
        }
    }

    /// Add `file` to the worklist if not already
    /// processed. Computes and caches the file's
    /// namespace.
    fn enqueue_file(&mut self, file: PathBuf) {
        // Linear scan: typical projects have <100 files
        // and a Vec scan is faster than hashing each
        // PathBuf. Mark the file as processed
        // immediately so concurrent enqueues from
        // `discover_all` don't re-add it.
        if self.processed.contains(&file) {
            #[cfg(debug_assertions)]
            eprintln!("[pipeline]   already loaded {}", file.display());
            return;
        }
        let ns = self.manifest.namespace_of(&self.project_root, &file);
        self.processed.push(file.clone());
        self.worklist.push_back(WorkItem {
            file: file.clone(),
            namespace: ns.clone(),
        });
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline]   enqueued {} (namespace={})",
            file.display(),
            ns.as_deref().unwrap_or("<none>")
        );
    }

    /// Read the source text for `file`, populating the
    /// `source_cache` so the second read (in
    /// `compile_file`) is a no-op. Returns `None` if
    /// the file can't be read; the caller records the
    /// error and bails.
    fn read_source(&mut self, file: &Path) -> Option<String> {
        // Intern the path. Repeated calls with the same
        // path return the same id; new paths extend the
        // interner's storage. The id is a `u32` (Copy),
        // not a `PathBuf` (heap-allocated), so the
        // lookup is cheaper than a HashMap key.
        let id = self.source_interner.intern(file.to_path_buf());
        // Resize the cache if this is a fresh path.
        // We extend Vec length up to (id + 1) with
        // `None` placeholders so the indexed lookup
        // below is bounds-checked by Rust (panics if
        // id is out of range, which it isn't by
        // construction).
        if self.source_cache.len() <= id {
            self.source_cache.resize(id + 1, None);
        }
        if let Some(cached) = self.source_cache[id].as_ref() {
            #[cfg(debug_assertions)]
            eprintln!("[pipeline]   cache hit for {}", file.display());
            return Some(cached.clone());
        }
        match std::fs::read_to_string(file) {
            Ok(s) => {
                #[cfg(debug_assertions)]
                eprintln!("[pipeline]   loaded {} ({} bytes)", file.display(), s.len());
                self.source_cache[id] = Some(s.clone());
                Some(s)
            }
            Err(_) => None,
        }
    }

    /// Discovery pass: walk the worklist front-to-back,
    /// parsing each file and enqueueing its
    /// `use`/`mod` dependencies. We don't compile
    /// here — just build the complete worklist so
    /// that the compilation pass can run in
    /// dependency order.
    ///
    /// The `processed` set guards against re-enqueuing
    /// (so the same file isn't discovered twice). The
    /// `failed` flag is set if any file fails to parse.
    fn discover_all(&mut self) {
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] scanning for files (entry={:?})",
            self.entry_file
        );
        // Walk the worklist from the front, parsing each
        // file to find its `use`/`mod` declarations.
        // `enqueue_file` adds new dependencies to the back
        // of the worklist and dedupes against `processed`,
        // so each file is scanned exactly once.
        //
        // Each scanned item is RE-ENQUEUED at the back so
        // the compile pass finds it. The trade-off:
        // O(N) extra pops (one per scan) vs allocating
        // a separate scan queue. For typical projects
        // (<100 files) the O(N) cost is negligible.
        //
        // `enqueue_uses`'s re-enqueues of already-processed
        // dependencies are no-ops, so the only repeated
        // work would be re-parsing a file's `use`s. We
        // skip that via `already_scanned` — a file's
        // `use`s are walked exactly once.
        //
        // Termination: track the worklist length at the
        // end of each pass. If it doesn't grow after a
        // pass (i.e., `enqueue_uses` added nothing new),
        // we're done. Each pass is at most one full
        // rotation of the worklist (since new items are
        // added to the BACK, the front gets recycled).
        // So total work is O(N^2) worst case, but in
        // practice O(N) for tree-shaped dependency
        // graphs.
        let mut already_scanned: Vec<PathBuf> = Vec::new();
        #[cfg(debug_assertions)]
        let mut depth = 0usize;
        loop {
            let item = match self.worklist.pop_front() {
                Some(i) => i,
                None => break,
            };
            let file = item.file.clone();
            if already_scanned.contains(&file) {
                // Re-enqueue at the back so the compile
                // pass finds it. But don't re-scan.
                self.worklist.push_back(item);
                if self
                    .worklist
                    .iter()
                    .all(|w| already_scanned.contains(&w.file))
                {
                    break;
                }
                continue;
            }
            #[cfg(debug_assertions)]
            {
                eprintln!("[pipeline]   scanning {} (depth {})", file.display(), depth);
                depth += 1;
            }
            already_scanned.push(file.clone());
            // Read the source (cached after the first
            // call). The `compile_file` pass reuses the
            // same cached source, so the file is only
            // read from disk once per pipeline.
            let src = match self.read_source(&file) {
                Some(s) => s,
                None => {
                    self.emit_spanless_error(
                        ErrorCode::IoError,
                        format!("Failed to read file `{}`", file.display()),
                    );
                    self.failed = true;
                    continue;
                }
            };
            let parser = Pratt::default();
            let ast = match parser.parse(src.as_str()) {
                Ok(ast) => ast,
                Err(errors) => {
                    // Emit once here. Do NOT re-enqueue: compile_file
                    // would parse again and duplicate the same report.
                    self.emit_message(&file, src.as_str(), &errors);
                    self.failed = true;
                    continue;
                }
            };
            // Re-enqueue only after a successful parse so the compile
            // pass drains the worklist in LIFO order via `pop_back`
            // (dependencies at the back are compiled first).
            self.worklist.push_back(item);
            self.enqueue_uses(&file, src.as_str(), &ast);
            // Only stop when every worklist entry has been
            // scanned. Length-stable checks alone are wrong:
            // scanning the first of two deps (`use a::*; use
            // b::*;`) adds nothing new while `b` is still
            // unscanned — glob expansion then sees an empty
            // functions table for that module.
            if self
                .worklist
                .iter()
                .all(|w| already_scanned.contains(&w.file))
            {
                break;
            }
        }
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] scanning complete, {} file(s) in worklist",
            self.worklist.len()
        );
    }

    /// Compile a single file: parse, enqueue uses, and
    /// invoke the compiler. Called once per WorkItem.
    fn compile_file(&mut self, item: WorkItem, is_entry: bool) {
        let file = item.file.clone();
        // The ENTRY file is special: it's the program root
        // and lives in the top-level namespace (no
        // prefix). Non-entry files get their path-derived
        // namespace so they can be referred to by their
        // fully qualified name (e.g., `builtins::core::ffi::dload`).
        let namespace = if is_entry {
            String::new()
        } else {
            item.namespace.unwrap_or_else(|| {
                // File is outside any search root. Use
                // the bare file stem as the namespace.
                file.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("anonymous")
                    .to_string()
            })
        };
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] compiling {} (namespace={:?}, entry={})",
            file.display(),
            namespace,
            is_entry
        );

        let src = match self.read_source(&file) {
            Some(s) => s,
            None => {
                self.emit_spanless_error(
                    ErrorCode::IoError,
                    format!("Failed to read file `{}`", file.display()),
                );
                self.failed = true;
                return;
            }
        };

        let parser = Pratt::default();
        let mut ast = match parser.parse(src.as_str()) {
            Ok(ast) => ast,
            Err(errors) => {
                self.emit_message(&file, src.as_str(), &errors);
                self.failed = true;
                return;
            }
        };

        // Note: `enqueue_uses` was already called by
        // `discover_all` in the pre-pass. The
        // worklist is fully populated. We just
        // compile now.

        let rel = file
            .strip_prefix(&self.project_root)
            .unwrap_or(&file)
            .to_path_buf();
        self.compiler.set_source_file(rel);

        // Compile the file. The compiler's `namespace`
        // field is set to the file's derived namespace.
        // We use `compile_module` (not `compile`) so the
        // returned bytes are ONLY the new bytes (not the
        // cumulative bytecode, which would duplicate
        // the prologue on the second call). See
        // `Compiler::compile_module` for the operand
        // adjustment details.
        let bytecode = self.compiler.compile_module(namespace.as_str(), &mut ast);
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline]   compiled {} → {} bytes (total: {})",
            file.display(),
            bytecode.len(),
            self.bytecode.len()
        );

        // Append this file's bytecode to the running
        // output. Each file's bytecode is independent;
        // the linker (the prologue's JMP) connects them
        // via function-name lookup at call time.
        self.bytecode.extend(bytecode);

        // Surface any newly emitted compiler diagnostics.
        let file_id = self.sink.register_source(&file, src.as_str());
        self.emit_new_messages(file_id);
        if self.had_errors() {
            self.failed = true;
        }
    }

    pub fn compile(mut self, filename: String, output: String) {
        // Seed the worklist with the entry file. The
        // entry is treated specially (top-level
        // namespace) — see `compile_file`.
        let entry = PathBuf::from(&filename);
        self.entry_file = Some(entry.clone());
        self.enqueue_file(entry);

        // Discovery pass: walk the dependency graph
        // transitively, enqueueing every referenced
        // file. We re-process the worklist, parsing
        // each file's AST to find its `use`/`mod`
        // declarations, but NOT compiling yet. This
        // builds the complete worklist so that the
        // compilation pass can run in dependency
        // order (dependencies first).
        self.discover_all();

        // Compilation pass: drain the worklist in
        // REVERSE order (LIFO via `pop_back`). The
        // `enqueue_file`/`enqueue_uses` ordering
        // means the LAST enqueued file is the
        // deepest dependency; popping from the back
        // gives us dependencies first. This guarantees
        // that when a file's `use foo::bar;` looks
        // up `foo::bar` in `self.functions`, the
        // function is already there.
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] compiling worklist ({} files, LIFO)",
            self.worklist.len()
        );
        while let Some(item) = self.worklist.pop_back() {
            let is_entry = self
                .entry_file
                .as_ref()
                .map(|e| *e == item.file)
                .unwrap_or(false);
            self.compile_file(item, is_entry);
        }

        if self.failed {
            return;
        }

        // Module compilation emits unfused absolute-offset bytecode.
        // Finalize (peephole fusion + CodePtr/MakePolyFn relocation) once
        // on the linked buffer, then sync the pipeline output.
        self.compiler.finalize_bytecode();
        self.bytecode = self.compiler.bytecode.clone();

        // Patch the JMP at offset 1 to point to the
        // user-program's `main`. If the source had at
        // least one `extern` block or module statics,
        // jump to `program_start_offset` so setup runs
        // before `main`. Otherwise jump straight to `main`.
        if let Some(byte) = self.bytecode.get_mut(1) {
            *byte = Byte::new(Instruction::JMP)
                .with_operand_u32(self.compiler.prologue_jmp_target());
        }

        // Wrap the bytecode in the versioned `ArchivedProgram` envelope
        // so that older `.hyc` files can be rejected at load time via
        // `version` mismatch (see `Pipeline::run`).
        let program = ArchivedProgram {
            version: ARCHIVE_VERSION,
            static_slot_count: self.compiler.static_slot_count(),
            constants: self.compiler.constants.clone(),
            bytecode: self.bytecode,
            source_files: self.compiler.source_files_list(),
            debug_locs: self.compiler.debug_locs().to_vec(),
        };

        let mut out = File::create(output).expect("Unable to open output file");
        let _ = out
            .write(
                rkyv::to_bytes::<rkyv::rancor::Error>(&program)
                    .unwrap()
                    .as_slice(),
            )
            .expect("Unable to write compiled output to file");
    }

    /// Compile a parsed AST and return the bytecode
    /// (ignoring typecheck messages). Used by the
    /// `fizbuz_runs_to_completion` golden test, which
    /// exercises a .hy example that the typechecker
    /// rejects (`return;` is parsed as a variable name)
    /// but the codegen still produces valid bytecode for.
    pub fn compile_test(
        &mut self,
        module: &str,
        ast: &mut (SimpleSpan, Box<Expression<'_>>),
    ) -> (Vec<Byte>, Vec<u64>) {
        let mut bytecode = self.compiler.compile(module, ast);

        // Patch the JMP at offset 1 (the second prologue
        // instruction).
        if let Some(byte) = bytecode.get_mut(1) {
            *byte = Byte::new(Instruction::JMP)
                .with_operand_u32(self.compiler.prologue_jmp_target());
        }

        (bytecode, self.compiler.constants.clone())
    }

    pub fn compile_src(&mut self, src: &str) -> Result<(Vec<Byte>, Vec<u64>), ()> {
        let parser = Pratt::default();
        let path = Path::new("<input>");
        let mut ast = match parser.parse(src) {
            Ok(ast) => ast,
            Err(err) => {
                self.emit_message(path, src, &err);
                return Err(());
            }
        };

        self.compiler.set_source_file(path);
        let mut bytecode = self.compiler.compile("", &mut ast);

        // Register source and drain typecheck / codegen diagnostics via the sink.
        let file_id = self.sink.register_source(path, src);
        self.emit_new_messages(file_id);
        if self.had_errors() {
            return Err(());
        }

        if let Some(byte) = bytecode.get_mut(1) {
            *byte = Byte::new(Instruction::JMP)
                .with_operand_u32(self.compiler.prologue_jmp_target());
        }

        Ok((bytecode, self.compiler.constants.clone()))
    }

    /// Compile a single source file in-memory and return the
    /// resulting bytecode, resolving `use` and `mod`
    /// declarations by reading the referenced files from disk.
    ///
    /// Multi-file entry point: discovers and compiles the module graph from disk.
    pub fn compile_src_from_file(&mut self, file: &str) -> Result<(Vec<Byte>, Vec<u64>), ()> {
        let entry = PathBuf::from(file);
        // Re-root the manifest from the entry file so
        // `cargo run -- examples/modules.hy` finds the workspace
        // `coil.toml` even when cwd differs.
        let root = Self::find_project_root(&entry);
        if root != self.project_root {
            self.project_root = root.clone();
            self.manifest = Manifest::load(&root).expect("Failed to load coil.toml for entry file");
            machine::env::set_allow_exec(self.manifest.allow_exec);
        }
        self.entry_file = Some(entry.clone());
        self.enqueue_file(entry);

        // Discovery + LIFO compile (see `compile`).
        self.discover_all();
        #[cfg(debug_assertions)]
        eprintln!(
            "[pipeline] compiling worklist ({} files, LIFO)",
            self.worklist.len()
        );
        while let Some(item) = self.worklist.pop_back() {
            let is_entry = self
                .entry_file
                .as_ref()
                .map(|e| *e == item.file)
                .unwrap_or(false);
            self.compile_file(item, is_entry);
        }

        if self.failed || self.had_errors() {
            return Err(());
        }

        // Final-link peephole fusion (see `Pipeline::compile`).
        self.compiler.finalize_bytecode();
        self.bytecode = self.compiler.bytecode.clone();

        // Patch the JMP at offset 1.
        if let Some(byte) = self.bytecode.get_mut(1) {
            *byte = Byte::new(Instruction::JMP)
                .with_operand_u32(self.compiler.prologue_jmp_target());
        }

        // In-memory API: any diagnostic (error or warning) is a failure.
        if !self.compiler.get_messages().is_empty() {
            return Err(());
        }

        Ok((
            std::mem::take(&mut self.bytecode),
            self.compiler.constants.clone(),
        ))
    }

    /// Harness test cases from the last compile (`description`, bytecode offset).
    pub fn test_cases(&self) -> &[(String, u32)] {
        self.compiler.test_cases()
    }

    /// When true, `test("…")` blocks and `#[test]` functions are compiled and
    /// registered for the harness. Default is false (production builds).
    pub fn set_include_tests(&mut self, include: bool) {
        self.include_tests = include;
        self.compiler.set_include_tests(include);
    }

    pub fn include_tests(&self) -> bool {
        self.include_tests
    }

    /// Borrow host-registered native function metadata.
    pub fn natives(&self) -> &[NativeDecl] {
        &self.natives
    }

    pub fn constants(&self) -> &[u64] {
        self.compiler.constants()
    }

    pub fn static_slot_count(&self) -> u32 {
        self.compiler.static_slot_count()
    }

    pub fn program_debug(&self) -> ProgramDebug {
        ProgramDebug {
            source_files: self.compiler.source_files_list(),
            debug_locs: self.compiler.debug_locs().to_vec(),
        }
    }

    pub fn run(self, filename: String) -> Result<(Vec<Byte>, Vec<u64>, u32, ProgramDebug), ()> {
        let mut f = File::open(filename).expect("Unable to find file");
        let mut buffer = Vec::with_capacity(1024);
        f.read_to_end(&mut buffer).expect("Unable to read file");

        // Access the archived envelope. Note: `ArchivedProgram` is the
        // SERIALIZABLE struct; rkyv's `Archive` derive generates a
        // separate archived struct named `ArchivedArchivedProgram`
        // (the derive just prepends `Archived` to the source name),
        // which is the type `rkyv::access` expects.
        let archived = rkyv::access::<ArchivedArchivedProgram, Error>(&buffer)
            .expect("Unable to decode rkyv binary");

        // Reject archives whose format doesn't match the in-tree
        // bytecode layout. `ARCHIVE_VERSION` is bumped whenever
        // `Byte` or any opcode changes incompatibly.
        if archived.version != ARCHIVE_VERSION {
            return Err(());
        }

        if self.failed {
            return Err(());
        }

        // Deserialize the archived `ArchivedVec<ArchivedByte>` back
        // into an owned `Vec<Byte>` for the VM. rkyv's `Deserialize`
        // impl for `ArchivedVec` handles the deep copy.
        let bytecode = rkyv::deserialize::<Vec<Byte>, Error>(&archived.bytecode)
            .expect("Unable to deserialize bytecode");
        let constants = rkyv::deserialize::<Vec<u64>, Error>(&archived.constants)
            .expect("Unable to deserialize constant pool");
        let static_slot_count = u32::from(archived.static_slot_count);
        let source_files = rkyv::deserialize::<Vec<String>, Error>(&archived.source_files)
            .expect("Unable to deserialize source_files");
        let debug_locs = rkyv::deserialize::<Vec<common::DebugLoc>, Error>(&archived.debug_locs)
            .expect("Unable to deserialize debug_locs");

        Ok((
            bytecode,
            constants,
            static_slot_count,
            ProgramDebug {
                source_files,
                debug_locs,
            },
        ))
    }
}

fn ffi_type_to_ty(ty: FfiType) -> crate::typechecking::ty::Ty {
    use crate::typechecking::ty::{array, boolean, float, int, string, unit};
    match ty {
        FfiType::Int
        | FfiType::Int8
        | FfiType::Int16
        | FfiType::Int32
        | FfiType::UInt8
        | FfiType::UInt16
        | FfiType::UInt32
        | FfiType::UInt64 => int(),
        FfiType::Float => float(),
        FfiType::String => string(),
        FfiType::Void => unit(),
        FfiType::Bool => boolean(),
        FfiType::Ptr => array(int()),
        FfiType::Callback(_) | FfiType::Struct(_) => int(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use reporting::{ErrorCode, Message, MessageKind, ReportConfig, ReportFormat};

    use super::Pipeline;

    /// Cloneable in-memory writer so tests can inspect sink output.
    #[derive(Clone, Default)]
    struct SharedBuf {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedBuf {
        fn new() -> Self {
            Self::default()
        }

        fn into_string(self) -> String {
            String::from_utf8_lossy(&self.inner.lock().unwrap()).into_owned()
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn with_reporter_emits_message_to_pretty_sink() {
        let shared = SharedBuf::new();
        let mut pipeline =
            Pipeline::with_reporter(ReportConfig::default(), Box::new(shared.clone()));

        // Type mismatch on assignment — should surface via the pretty sink.
        let src = r#"
fn main() {
    let x = 1;
    x = "hi";
}
"#;
        let result = pipeline.compile_src(src);
        assert!(result.is_err());
        pipeline.finish_reporting().unwrap();

        let out = shared.into_string();
        assert!(
            out.contains("Type mismatch") || out.contains("E0102"),
            "expected type-mismatch diagnostic in sink output, got: {out:?}"
        );
        // Also exercise the E01xx family path with an unknown function.
        let shared2 = SharedBuf::new();
        let mut pipeline2 =
            Pipeline::with_reporter(ReportConfig::default(), Box::new(shared2.clone()));
        let _ = pipeline2.compile_src("fn main() { nope(); }");
        pipeline2.finish_reporting().unwrap();
        let out2 = shared2.into_string();
        assert!(
            out2.contains("E0101") || out2.contains("Cannot find function"),
            "expected E0101 / unknown-function diagnostic, got: {out2:?}"
        );
        assert!(pipeline.had_errors());
        assert!(pipeline2.had_errors());
    }

    #[test]
    fn emit_spanless_error_records_error() {
        let shared = SharedBuf::new();
        let mut pipeline =
            Pipeline::with_reporter(ReportConfig::default(), Box::new(shared.clone()));
        pipeline.emit_spanless_error(ErrorCode::IoError, "failed to open archive");
        assert!(pipeline.had_errors());
        pipeline.finish_reporting().unwrap();

        let out = shared.into_string();
        assert!(out.contains("failed to open archive"));
        assert!(out.contains("E0900") || out.contains("error"));
    }

    #[test]
    fn message_kind_still_distinguishes_error_and_warning() {
        let err = Message::error(ErrorCode::TypeMismatch, "boom".into(), 0..1);
        let warn = Message::warn(ErrorCode::UnknownValue, "unused".into(), 0..1);
        assert_eq!(*err.kind(), MessageKind::ERROR);
        assert_eq!(*warn.kind(), MessageKind::WARNING);
    }

    #[test]
    fn create_sink_sarif_round_trip() {
        let shared = SharedBuf::new();
        let config = ReportConfig::new(ReportFormat::Sarif);
        let mut pipeline = Pipeline::with_reporter(config, Box::new(shared.clone()));

        // Unknown value → E0100.
        let src = r#"fn main() { print "%i", missing; }"#;
        let _ = pipeline.compile_src(src);
        pipeline.finish_reporting().unwrap();

        let out = shared.into_string();
        assert!(
            out.contains("E0100") || out.contains(r#""ruleId":"E0100"#),
            "expected SARIF ruleId E0100, got: {out:?}"
        );
        assert!(pipeline.had_errors());
    }
}
