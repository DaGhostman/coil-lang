//! Compiler-provided virtual modules (`prelude`, `ffi`, …).
//!
//! These are not `.hy` files on disk. `use` resolves against this
//! registry before falling back to [`crate::manifest::Manifest`] path
//! discovery, and every file gets an implicit prelude import.

use std::collections::HashMap;

/// Canonical module path for Option / Result.
pub const PRELUDE_MODULE: &str = "prelude";

/// Canonical module path for operator / comparison traits.
pub const PRELUDE_OPS_MODULE: &str = "prelude::ops";

/// Canonical module path for FFI callables (`dload` / `declare` / `invoke`).
pub const FFI_MODULE: &str = "ffi";

/// Canonical module path for FFI type-tag constructors (`Int`, `Ptr`, …).
pub const FFI_TYPES_MODULE: &str = "ffi::types";

/// Canonical module path for test helpers (`assert`).
pub const PRELUDE_TEST_MODULE: &str = "prelude::test";

/// Canonical module path for linear-algebra helpers (`dot` / `matmul` / `cross`).
pub const PRELUDE_MATH_MODULE: &str = "prelude::math";

/// Canonical module path for IO streams (`open`, `read`, `Stream`, …).
pub const IO_MODULE: &str = "io";

/// TCP helpers under `io::net::tcp` (`connect`, `listen`, …).
pub const IO_NET_TCP_MODULE: &str = "io::net::tcp";

/// UDP helpers under `io::net::udp` (`bind`, `send_to`, …).
pub const IO_NET_UDP_MODULE: &str = "io::net::udp";

/// TLS helpers under `io::net::tls` (`connect`, `connect_insecure`).
#[cfg(feature = "tls")]
pub const IO_NET_TLS_MODULE: &str = "io::net::tls";

/// Canonical module path for OS threads, channels, and locks.
pub const THREAD_MODULE: &str = "thread";

/// Path-oriented filesystem helpers (`exists`, `realpath`, …).
pub const IO_FS_MODULE: &str = "io::fs";

/// Wall clock, periods, and formatting (`timestamp`, `sleep_ms`, …).
#[cfg(feature = "time")]
pub const TIME_MODULE: &str = "time";

/// Process environment (`args`, `var`, `exec`, …).
pub const ENV_MODULE: &str = "env";

/// Cryptographic primitives (`sha256`, `random_bytes`, …).
#[cfg(feature = "crypto")]
pub const CRYPTO_MODULE: &str = "crypto";

/// PCRE2 regex (`compile`, `is_match`, `find_all`, …).
#[cfg(feature = "regex")]
pub const REGEX_MODULE: &str = "regex";

/// Which userland FFI builtin a virtual export names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FfiBuiltin {
    Dload,
    Declare,
    Invoke,
}

impl FfiBuiltin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dload => "dload",
            Self::Declare => "declare",
            Self::Invoke => "invoke",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dload" => Some(Self::Dload),
            "declare" => Some(Self::Declare),
            "invoke" => Some(Self::Invoke),
            _ => None,
        }
    }
}

/// Prelude/test callables exported from virtual modules (parallel to [`FfiBuiltin`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreludeFn {
    Assert,
    Dot,
    MatMul,
    Cross,
    /// Construct a nominal `Matrix` from nested static rows.
    Matrix,
    /// Construct a single UTF-8 code unit as `Result<string, string>`.
    Char,
    /// First code unit of a `string` as `Result<byte, string>`.
    Ord,
}

impl PreludeFn {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assert => "assert",
            Self::Dot => "dot",
            Self::MatMul => "matmul",
            Self::Cross => "cross",
            Self::Matrix => "matrix",
            Self::Char => "char",
            Self::Ord => "ord",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "assert" => Some(Self::Assert),
            "dot" => Some(Self::Dot),
            "matmul" => Some(Self::MatMul),
            "cross" => Some(Self::Cross),
            "matrix" => Some(Self::Matrix),
            "char" => Some(Self::Char),
            "ord" => Some(Self::Ord),
            _ => None,
        }
    }
}

/// IO host natives exported from virtual `io` / `io::net::*` modules.
///
/// Surface names (after `use`) are short (`bind`, `connect`). Host registry
/// keys stay uniquely prefixed (`udp_bind`, `tcp_connect`) so TCP and UDP
/// never collide in [`Compiler::native`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IoBuiltin {
    Stdin,
    Stdout,
    Stderr,
    Open,
    Close,
    Read,
    Write,
    ReadExact,
    ReadToEnd,
    WriteAll,
    /// Decode `[byte]` as UTF-8 → `Result<string, IoError>`.
    FromBytes,
    /// Encode `string` → `[byte]` (UTF-8).
    ToBytes,
    TcpConnect,
    TcpListen,
    TcpAccept,
    TcpAcceptWait,
    /// Bind a UDP datagram socket (`host`, `port`; `port` may be `0`).
    UdpBind,
    /// Create a connected UDP socket toward (`host`, `port`).
    UdpConnect,
    /// Send a datagram to an explicit peer.
    UdpSendTo,
    /// Non-blocking recv; returns `(nbytes, peer_host, peer_port)`.
    UdpRecvFrom,
    /// Block until a datagram arrives (host `poll`).
    UdpRecvFromWait,
    /// Local bound port of a UDP socket (useful after `bind(..., 0)`).
    UdpLocalPort,
    /// TLS connect with webpki roots + SNI (`io::net::tls::connect`).
    #[cfg(feature = "tls")]
    TlsConnect,
    /// TLS connect without certificate verification.
    #[cfg(feature = "tls")]
    TlsConnectInsecure,
}

impl IoBuiltin {
    /// Name bound by `use` / shown in diagnostics (`bind`, `connect`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdin => "stdin",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Open => "open",
            Self::Close => "close",
            Self::Read => "read",
            Self::Write => "write",
            Self::ReadExact => "read_exact",
            Self::ReadToEnd => "read_to_end",
            Self::WriteAll => "write_all",
            Self::FromBytes => "from_bytes",
            Self::ToBytes => "to_bytes",
            Self::TcpConnect => "connect",
            Self::TcpListen => "listen",
            Self::TcpAccept => "accept",
            Self::TcpAcceptWait => "accept_wait",
            Self::UdpBind => "bind",
            Self::UdpConnect => "connect",
            Self::UdpSendTo => "send_to",
            Self::UdpRecvFrom => "recv_from",
            Self::UdpRecvFromWait => "recv_from_wait",
            Self::UdpLocalPort => "local_port",
            #[cfg(feature = "tls")]
            Self::TlsConnect => "connect",
            #[cfg(feature = "tls")]
            Self::TlsConnectInsecure => "connect_insecure",
        }
    }

    /// Stable host-native registry key (unique across TCP/UDP).
    pub fn native_name(self) -> &'static str {
        match self {
            Self::Stdin
            | Self::Stdout
            | Self::Stderr
            | Self::Open
            | Self::Close
            | Self::Read
            | Self::Write
            | Self::ReadExact
            | Self::ReadToEnd
            | Self::WriteAll
            | Self::FromBytes
            | Self::ToBytes => self.as_str(),
            Self::TcpConnect => "tcp_connect",
            Self::TcpListen => "tcp_listen",
            Self::TcpAccept => "tcp_accept",
            Self::TcpAcceptWait => "tcp_accept_wait",
            Self::UdpBind => "udp_bind",
            Self::UdpConnect => "udp_connect",
            Self::UdpSendTo => "udp_send_to",
            Self::UdpRecvFrom => "udp_recv_from",
            Self::UdpRecvFromWait => "udp_recv_from_wait",
            Self::UdpLocalPort => "udp_local_port",
            #[cfg(feature = "tls")]
            Self::TlsConnect => "tls_connect",
            #[cfg(feature = "tls")]
            Self::TlsConnectInsecure => "tls_connect_insecure",
        }
    }

    /// Core stream / file / text helpers on the top-level `io` module.
    pub fn core() -> &'static [IoBuiltin] {
        &[
            Self::Stdin,
            Self::Stdout,
            Self::Stderr,
            Self::Open,
            Self::Close,
            Self::Read,
            Self::Write,
            Self::ReadExact,
            Self::ReadToEnd,
            Self::WriteAll,
            Self::FromBytes,
            Self::ToBytes,
        ]
    }

    /// Exports of `io::net::tcp`.
    pub fn tcp() -> &'static [IoBuiltin] {
        &[
            Self::TcpConnect,
            Self::TcpListen,
            Self::TcpAccept,
            Self::TcpAcceptWait,
        ]
    }

    /// Exports of `io::net::udp`.
    pub fn udp() -> &'static [IoBuiltin] {
        &[
            Self::UdpBind,
            Self::UdpConnect,
            Self::UdpSendTo,
            Self::UdpRecvFrom,
            Self::UdpRecvFromWait,
            Self::UdpLocalPort,
        ]
    }

    /// Exports of `io::net::tls`.
    #[cfg(feature = "tls")]
    pub fn tls() -> &'static [IoBuiltin] {
        &[Self::TlsConnect, Self::TlsConnectInsecure]
    }

    /// Every IO host native (for pipeline registration).
    pub fn all() -> &'static [IoBuiltin] {
        &[
            Self::Stdin,
            Self::Stdout,
            Self::Stderr,
            Self::Open,
            Self::Close,
            Self::Read,
            Self::Write,
            Self::ReadExact,
            Self::ReadToEnd,
            Self::WriteAll,
            Self::FromBytes,
            Self::ToBytes,
            Self::TcpConnect,
            Self::TcpListen,
            Self::TcpAccept,
            Self::TcpAcceptWait,
            Self::UdpBind,
            Self::UdpConnect,
            Self::UdpSendTo,
            Self::UdpRecvFrom,
            Self::UdpRecvFromWait,
            Self::UdpLocalPort,
            #[cfg(feature = "tls")]
            Self::TlsConnect,
            #[cfg(feature = "tls")]
            Self::TlsConnectInsecure,
        ]
    }
}

/// Thread host natives exported from virtual `thread`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadBuiltin {
    Spawn,
    Join,
    Detach,
    Channel,
    Send,
    Recv,
    TrySend,
    TryRecv,
    Close,
    Mutex,
    WithLock,
    Lock,
    TryLock,
    Unlock,
    Rwlock,
    WithRead,
    WithWrite,
    TryRead,
    TryWrite,
}

impl ThreadBuiltin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Join => "join",
            Self::Detach => "detach",
            Self::Channel => "channel",
            Self::Send => "send",
            Self::Recv => "recv",
            Self::TrySend => "try_send",
            Self::TryRecv => "try_recv",
            Self::Close => "close",
            Self::Mutex => "mutex",
            Self::WithLock => "with_lock",
            Self::Lock => "lock",
            Self::TryLock => "try_lock",
            Self::Unlock => "unlock",
            Self::Rwlock => "rwlock",
            Self::WithRead => "with_read",
            Self::WithWrite => "with_write",
            Self::TryRead => "try_read",
            Self::TryWrite => "try_write",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Self::Spawn => "thread_spawn",
            Self::Join => "thread_join",
            Self::Detach => "thread_detach",
            Self::Channel => "thread_channel",
            Self::Send => "thread_send",
            Self::Recv => "thread_recv",
            Self::TrySend => "thread_try_send",
            Self::TryRecv => "thread_try_recv",
            Self::Close => "thread_close",
            Self::Mutex => "thread_mutex",
            Self::WithLock => "thread_with_lock",
            Self::Lock => "thread_lock",
            Self::TryLock => "thread_try_lock",
            Self::Unlock => "thread_unlock",
            Self::Rwlock => "thread_rwlock",
            Self::WithRead => "thread_with_read",
            Self::WithWrite => "thread_with_write",
            Self::TryRead => "thread_try_read",
            Self::TryWrite => "thread_try_write",
        }
    }

    pub fn all() -> &'static [ThreadBuiltin] {
        &[
            Self::Spawn,
            Self::Join,
            Self::Detach,
            Self::Channel,
            Self::Send,
            Self::Recv,
            Self::TrySend,
            Self::TryRecv,
            Self::Close,
            Self::Mutex,
            Self::WithLock,
            Self::Lock,
            Self::TryLock,
            Self::Unlock,
            Self::Rwlock,
            Self::WithRead,
            Self::WithWrite,
            Self::TryRead,
            Self::TryWrite,
        ]
    }
}

/// One item exported by a virtual module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinExport {
    /// Built-in sum type (`Option`, `Result`). Internal registry key is `name`.
    Enum { name: &'static str },
    /// Built-in typeclass (`Eq`, `Num`, …). Internal key is `name`.
    TypeClass { name: &'static str },
    /// FFI tag constructor (`Int`, `Ptr`, …) → same tags as historical `FFIType::X`.
    FfiTag { variant: &'static str },
    /// Userland FFI callable.
    FfiFn { kind: FfiBuiltin },
    /// Prelude/test callable (`assert`, …).
    Fn { kind: PreludeFn },
    /// Opaque built-in type name (`Stream`).
    OpaqueType { name: &'static str },
    /// IO host native (`open`, `read`, …).
    IoFn { kind: IoBuiltin },
    /// Thread host native (`spawn`, `send`, …).
    ThreadFn { kind: ThreadBuiltin },
    /// Generic pipeline host native (`registry` key for [`HostInvoke`]).
    HostFn {
        surface: &'static str,
        registry: &'static str,
    },
}

impl BuiltinExport {
    pub fn short_name(&self) -> &str {
        match self {
            Self::Enum { name } => name,
            Self::TypeClass { name } => name,
            Self::FfiTag { variant } => variant,
            Self::FfiFn { kind } => kind.as_str(),
            Self::Fn { kind } => kind.as_str(),
            Self::OpaqueType { name } => name,
            Self::IoFn { kind } => kind.as_str(),
            Self::ThreadFn { kind } => kind.as_str(),
            Self::HostFn { surface, .. } => surface,
        }
    }

    pub fn host_registry(&self) -> Option<&'static str> {
        match self {
            Self::HostFn { registry, .. } => Some(registry),
            _ => None,
        }
    }
}

/// Path → exports for compiler virtual modules.
#[derive(Debug, Clone)]
pub struct VirtualModules {
    modules: HashMap<&'static str, Vec<BuiltinExport>>,
}

impl Default for VirtualModules {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualModules {
    pub fn new() -> Self {
        let mut modules: HashMap<&'static str, Vec<BuiltinExport>> = HashMap::new();

        modules.insert(
            PRELUDE_MODULE,
            vec![
                BuiltinExport::Enum {
                    name: common::BUILTIN_OPTION_ENUM,
                },
                BuiltinExport::Enum {
                    name: common::BUILTIN_RESULT_ENUM,
                },
                BuiltinExport::TypeClass { name: "Iterator" },
                BuiltinExport::TypeClass {
                    name: "IntoIterator",
                },
                BuiltinExport::Fn {
                    kind: PreludeFn::Ord,
                },
                BuiltinExport::Fn {
                    kind: PreludeFn::Char,
                },
            ],
        );

        modules.insert(
            PRELUDE_OPS_MODULE,
            vec![
                BuiltinExport::TypeClass { name: "Add" },
                BuiltinExport::TypeClass { name: "Sub" },
                BuiltinExport::TypeClass { name: "Mul" },
                BuiltinExport::TypeClass { name: "Div" },
                BuiltinExport::TypeClass { name: "Num" },
                BuiltinExport::TypeClass { name: "Eq" },
                BuiltinExport::TypeClass { name: "Ord" },
                BuiltinExport::TypeClass { name: "Lt" },
                BuiltinExport::TypeClass { name: "Le" },
                BuiltinExport::TypeClass { name: "Gt" },
                BuiltinExport::TypeClass { name: "Ge" },
                BuiltinExport::TypeClass { name: "Show" },
                BuiltinExport::TypeClass { name: "Into" },
            ],
        );

        modules.insert(
            FFI_MODULE,
            vec![
                BuiltinExport::Enum {
                    name: common::BUILTIN_FFI_ERROR_ENUM,
                },
                BuiltinExport::Enum {
                    name: common::BUILTIN_FFI_ERROR_KIND_ENUM,
                },
                BuiltinExport::FfiFn {
                    kind: FfiBuiltin::Dload,
                },
                BuiltinExport::FfiFn {
                    kind: FfiBuiltin::Declare,
                },
                BuiltinExport::FfiFn {
                    kind: FfiBuiltin::Invoke,
                },
            ],
        );

        let ffi_tags: Vec<BuiltinExport> = common::BUILTIN_FFI_TYPE_VARIANTS
            .iter()
            .map(|variant| BuiltinExport::FfiTag { variant })
            .collect();
        modules.insert(FFI_TYPES_MODULE, ffi_tags);

        modules.insert(
            PRELUDE_TEST_MODULE,
            vec![BuiltinExport::Fn {
                kind: PreludeFn::Assert,
            }],
        );

        modules.insert(
            PRELUDE_MATH_MODULE,
            vec![
                BuiltinExport::Fn {
                    kind: PreludeFn::Dot,
                },
                BuiltinExport::Fn {
                    kind: PreludeFn::MatMul,
                },
                BuiltinExport::Fn {
                    kind: PreludeFn::Cross,
                },
                BuiltinExport::Fn {
                    kind: PreludeFn::Matrix,
                },
                BuiltinExport::OpaqueType {
                    name: common::BUILTIN_MATRIX_TYPE,
                },
            ],
        );

        let mut io_exports = vec![
            BuiltinExport::OpaqueType { name: "Stream" },
            BuiltinExport::Enum {
                name: common::BUILTIN_IO_ERROR_ENUM,
            },
            BuiltinExport::TypeClass { name: "Read" },
            BuiltinExport::TypeClass { name: "Write" },
        ];
        for kind in IoBuiltin::core() {
            io_exports.push(BuiltinExport::IoFn { kind: *kind });
        }
        modules.insert(IO_MODULE, io_exports);

        let tcp_exports: Vec<BuiltinExport> = IoBuiltin::tcp()
            .iter()
            .map(|kind| BuiltinExport::IoFn { kind: *kind })
            .collect();
        modules.insert(IO_NET_TCP_MODULE, tcp_exports);

        let udp_exports: Vec<BuiltinExport> = IoBuiltin::udp()
            .iter()
            .map(|kind| BuiltinExport::IoFn { kind: *kind })
            .collect();
        modules.insert(IO_NET_UDP_MODULE, udp_exports);

        #[cfg(feature = "tls")]
        {
            let tls_exports: Vec<BuiltinExport> = IoBuiltin::tls()
                .iter()
                .map(|kind| BuiltinExport::IoFn { kind: *kind })
                .collect();
            modules.insert(IO_NET_TLS_MODULE, tls_exports);
        }

        let mut thread_exports = vec![
            BuiltinExport::OpaqueType { name: "Thread" },
            BuiltinExport::OpaqueType { name: "Sender" },
            BuiltinExport::OpaqueType { name: "Receiver" },
            BuiltinExport::OpaqueType { name: "Mutex" },
            BuiltinExport::OpaqueType { name: "RwLock" },
            BuiltinExport::Enum {
                name: common::BUILTIN_THREAD_ERROR_ENUM,
            },
        ];
        for kind in ThreadBuiltin::all() {
            thread_exports.push(BuiltinExport::ThreadFn { kind: *kind });
        }
        modules.insert(THREAD_MODULE, thread_exports);

        fn host_exports(pairs: &[(&'static str, &'static str)]) -> Vec<BuiltinExport> {
            pairs
                .iter()
                .map(|(surface, registry)| BuiltinExport::HostFn {
                    surface,
                    registry,
                })
                .collect()
        }

        modules.insert(
            IO_FS_MODULE,
            host_exports(&[
                ("exists", "fs_exists"),
                ("is_file", "fs_is_file"),
                ("is_dir", "fs_is_dir"),
                ("is_symlink", "fs_is_symlink"),
                ("metadata", "fs_metadata"),
                ("create_dir", "fs_create_dir"),
                ("create_dir_all", "fs_create_dir_all"),
                ("remove_file", "fs_remove_file"),
                ("remove_dir", "fs_remove_dir"),
                ("remove_dir_all", "fs_remove_dir_all"),
                ("rename", "fs_rename"),
                ("copy", "fs_copy"),
                ("read_link", "fs_read_link"),
                ("symlink", "fs_symlink"),
                ("list_dir", "fs_list_dir"),
                ("realpath", "fs_realpath"),
            ]),
        );

        let mut env_exports = vec![BuiltinExport::Enum {
            name: common::BUILTIN_ENV_ERROR_ENUM,
        }];
        env_exports.extend(host_exports(&[
            ("args", "env_args"),
            ("var", "env_var"),
            ("set_var", "env_set_var"),
            ("remove_var", "env_remove_var"),
            ("cwd", "env_cwd"),
            ("set_cwd", "env_set_cwd"),
            ("exit", "env_exit"),
            ("exec", "env_exec"),
        ]));
        modules.insert(ENV_MODULE, env_exports);

        #[cfg(feature = "time")]
        {
            let mut time_exports = vec![BuiltinExport::Enum {
                name: common::BUILTIN_TIME_ERROR_ENUM,
            }];
            time_exports.extend(host_exports(&[
                ("timestamp", "time_timestamp"),
                ("sleep_ms", "time_sleep_ms"),
                ("instant_now", "time_instant_now"),
                ("elapsed_nanos", "time_elapsed_nanos"),
                ("elapsed_millis", "time_elapsed_millis"),
                ("period", "time_period"),
                ("add", "time_add"),
                ("sub", "time_sub"),
                ("period_add", "time_period_add"),
                ("period_sub", "time_period_sub"),
                ("date", "time_date"),
                ("date_from_period", "time_date_from_period"),
                ("date_from_epoch_period", "time_date_from_epoch_period"),
                ("epoch", "time_epoch"),
                ("format", "time_format"),
                ("parse", "time_parse"),
            ]));
            modules.insert(TIME_MODULE, time_exports);
        }

        #[cfg(feature = "crypto")]
        {
            let mut crypto_exports = vec![BuiltinExport::Enum {
                name: common::BUILTIN_CRYPTO_ERROR_ENUM,
            }];
            crypto_exports.extend(host_exports(&[
                ("sha256", "crypto_sha256"),
                ("sha512", "crypto_sha512"),
                ("blake3", "crypto_blake3"),
                ("init", "crypto_hasher_init"),
                ("update", "crypto_hasher_update"),
                ("finalize", "crypto_hasher_finalize"),
                ("hmac_sha256", "crypto_hmac_sha256"),
                ("hmac_sha512", "crypto_hmac_sha512"),
                ("hmac_verify_sha256", "crypto_hmac_verify_sha256"),
                ("random_bytes", "crypto_random_bytes"),
                ("random_u64", "crypto_random_u64"),
                (
                    "chacha20_poly1305_encrypt",
                    "crypto_chacha20_poly1305_encrypt",
                ),
                (
                    "chacha20_poly1305_decrypt",
                    "crypto_chacha20_poly1305_decrypt",
                ),
                ("aes_256_gcm_encrypt", "crypto_aes_256_gcm_encrypt"),
                ("aes_256_gcm_decrypt", "crypto_aes_256_gcm_decrypt"),
                ("ed25519_generate", "crypto_ed25519_generate"),
                ("ed25519_sign", "crypto_ed25519_sign"),
                ("ed25519_verify", "crypto_ed25519_verify"),
                ("x25519_generate", "crypto_x25519_generate"),
                ("x25519_shared_secret", "crypto_x25519_shared_secret"),
                ("argon2id_hash", "crypto_argon2id_hash"),
                ("argon2id_verify", "crypto_argon2id_verify"),
                ("ct_eq", "crypto_ct_eq"),
            ]));
            modules.insert(CRYPTO_MODULE, crypto_exports);
        }

        #[cfg(feature = "regex")]
        {
            let mut regex_exports = vec![
                BuiltinExport::OpaqueType { name: "Regex" },
                BuiltinExport::Enum {
                    name: common::BUILTIN_REGEX_ERROR_ENUM,
                },
            ];
            regex_exports.extend(host_exports(&[
                ("compile", "regex_compile"),
                ("is_match", "regex_is_match"),
                ("find", "regex_find"),
                ("find_all", "regex_find_all"),
                ("captures", "regex_captures"),
                ("captures_all", "regex_captures_all"),
                ("split", "regex_split"),
                ("replace", "regex_replace"),
                ("replace_all", "regex_replace_all"),
            ]));
            modules.insert(REGEX_MODULE, regex_exports);
        }

        Self { modules }
    }

    /// True when `module_path` is a known virtual module (`"prelude"`, `"ffi::types"`, …).
    pub fn is_virtual_module(&self, module_path: &str) -> bool {
        self.modules.contains_key(module_path)
    }

    /// Join `use` path segments (+ optional final item that is not `*`) into a module path.
    pub fn module_path_of(path: &[String], name: &str) -> String {
        if name == "*" {
            path.join("::")
        } else if path.is_empty() {
            name.to_string()
        } else {
            format!("{}::{}", path.join("::"), name)
        }
    }

    /// Resolve a concrete `use path::name` (not glob) against virtual modules.
    ///
    /// `path` is the directory segments; `name` is the last segment (item).
    /// For `use prelude::ops::Eq`, path=`["prelude","ops"]`, name=`"Eq"`.
    pub fn resolve_item(&self, path: &[String], name: &str) -> Option<BuiltinExport> {
        if name == "*" {
            return None;
        }
        let module = path.join("::");
        self.modules
            .get(module.as_str())?
            .iter()
            .find(|e| e.short_name() == name)
            .cloned()
    }

    /// Resolve `use module::*` — returns every export of that module.
    pub fn resolve_glob(&self, path: &[String]) -> Option<&[BuiltinExport]> {
        let module = path.join("::");
        self.modules.get(module.as_str()).map(|v| v.as_slice())
    }

    /// True when this `use` targets a virtual module (concrete or glob).
    ///
    /// Used by the pipeline to skip disk discovery.
    pub fn resolves_use(&self, path: &[String], name: &str) -> bool {
        if name == "*" {
            self.resolve_glob(path).is_some()
        } else {
            self.resolve_item(path, name).is_some()
        }
    }

    /// Exports injected into every file (implicit
    /// `use prelude::*; use prelude::ops::*; use prelude::test::*; use prelude::math::*;`).
    pub fn prelude_exports(&self) -> Vec<BuiltinExport> {
        let mut out = Vec::new();
        if let Some(e) = self.modules.get(PRELUDE_MODULE) {
            out.extend(e.iter().cloned());
        }
        if let Some(e) = self.modules.get(PRELUDE_OPS_MODULE) {
            out.extend(e.iter().cloned());
        }
        if let Some(e) = self.modules.get(PRELUDE_TEST_MODULE) {
            out.extend(e.iter().cloned());
        }
        if let Some(e) = self.modules.get(PRELUDE_MATH_MODULE) {
            out.extend(e.iter().cloned());
        }
        out
    }

    /// Look up a typeclass by qualified path (`prelude::ops::Eq` → `"Eq"`).
    pub fn resolve_typeclass_path(&self, segments: &[&str]) -> Option<&'static str> {
        if segments.len() < 2 {
            return None;
        }
        let (module_segs, name) = segments.split_at(segments.len() - 1);
        let module = module_segs.join("::");
        match self.modules.get(module.as_str())?.iter().find(|e| {
            matches!(e, BuiltinExport::TypeClass { name: n } if n == &name[0])
        })? {
            BuiltinExport::TypeClass { name } => Some(*name),
            _ => None,
        }
    }

    /// Look up an enum by qualified path (`prelude::Option` → `"Option"`).
    pub fn resolve_enum_path(&self, segments: &[&str]) -> Option<&'static str> {
        if segments.len() < 2 {
            return None;
        }
        let (module_segs, name) = segments.split_at(segments.len() - 1);
        let module = module_segs.join("::");
        match self.modules.get(module.as_str())?.iter().find(|e| {
            matches!(e, BuiltinExport::Enum { name: n } if n == &name[0])
        })? {
            BuiltinExport::Enum { name } => Some(*name),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prelude_exports_option_result_and_ops() {
        let vm = VirtualModules::new();
        let exports = vm.prelude_exports();
        assert!(
            exports
                .iter()
                .any(|e| matches!(e, BuiltinExport::Enum { name: "Option" }))
        );
        assert!(
            exports
                .iter()
                .any(|e| matches!(e, BuiltinExport::TypeClass { name: "Eq" }))
        );
        assert!(
            exports
                .iter()
                .any(|e| matches!(e, BuiltinExport::TypeClass { name: "Into" }))
        );
        assert!(
            exports.iter().any(|e| matches!(
                e,
                BuiltinExport::Fn {
                    kind: PreludeFn::Assert
                }
            ))
        );
        assert!(
            exports.iter().any(|e| matches!(
                e,
                BuiltinExport::Fn {
                    kind: PreludeFn::Dot
                }
            ))
        );
        assert!(
            !exports
                .iter()
                .any(|e| matches!(e, BuiltinExport::FfiFn { .. }))
        );
    }

    #[test]
    fn resolve_concrete_prelude_test_assert() {
        let vm = VirtualModules::new();
        let e = vm
            .resolve_item(&["prelude".into(), "test".into()], "assert")
            .expect("prelude::test::assert");
        assert_eq!(
            e,
            BuiltinExport::Fn {
                kind: PreludeFn::Assert
            }
        );
        assert!(vm.resolves_use(&["prelude".into(), "test".into()], "*"));
    }

    #[test]
    fn ffi_types_glob_lists_tags() {
        let vm = VirtualModules::new();
        let tags = vm
            .resolve_glob(&["ffi".into(), "types".into()])
            .expect("ffi::types");
        assert!(
            tags.iter()
                .any(|e| matches!(e, BuiltinExport::FfiTag { variant: "Int" }))
        );
        assert!(
            tags.iter()
                .any(|e| matches!(e, BuiltinExport::FfiTag { variant: "Ptr" }))
        );
    }

    #[test]
    fn resolve_concrete_ffi_dload() {
        let vm = VirtualModules::new();
        let e = vm
            .resolve_item(&["ffi".into()], "dload")
            .expect("ffi::dload");
        assert_eq!(
            e,
            BuiltinExport::FfiFn {
                kind: FfiBuiltin::Dload
            }
        );
    }

    #[test]
    fn io_net_udp_exports_short_names_not_prefixed() {
        let vm = VirtualModules::new();
        let exports = vm
            .resolve_glob(&["io".into(), "net".into(), "udp".into()])
            .expect("io::net::udp");
        assert!(exports.iter().any(|e| e.short_name() == "bind"));
        assert!(exports.iter().any(|e| e.short_name() == "send_to"));
        assert!(exports.iter().any(|e| e.short_name() == "recv_from_wait"));
        assert!(!exports.iter().any(|e| e.short_name() == "udp_bind"));

        let bind = vm
            .resolve_item(
                &["io".into(), "net".into(), "udp".into()],
                "bind",
            )
            .expect("io::net::udp::bind");
        assert_eq!(
            bind,
            BuiltinExport::IoFn {
                kind: IoBuiltin::UdpBind
            }
        );
        assert_eq!(IoBuiltin::UdpBind.native_name(), "udp_bind");
        assert_eq!(IoBuiltin::TcpConnect.as_str(), "connect");
        assert_eq!(IoBuiltin::TcpConnect.native_name(), "tcp_connect");
    }

    #[cfg(feature = "tls")]
    #[test]
    fn io_net_tls_exports_connect_and_insecure() {
        let vm = VirtualModules::new();
        let exports = vm
            .resolve_glob(&["io".into(), "net".into(), "tls".into()])
            .expect("io::net::tls");
        assert!(exports.iter().any(|e| e.short_name() == "connect"));
        assert!(exports.iter().any(|e| e.short_name() == "connect_insecure"));
        assert_eq!(IoBuiltin::TlsConnect.native_name(), "tls_connect");
        assert_eq!(
            IoBuiltin::TlsConnectInsecure.native_name(),
            "tls_connect_insecure"
        );
        assert_eq!(IoBuiltin::TlsConnect.as_str(), "connect");
        assert_eq!(IoBuiltin::TlsConnectInsecure.as_str(), "connect_insecure");
    }

    #[test]
    fn io_glob_excludes_net_helpers() {
        let vm = VirtualModules::new();
        let exports = vm.resolve_glob(&["io".into()]).expect("io");
        assert!(exports.iter().any(|e| e.short_name() == "open"));
        assert!(exports.iter().any(|e| e.short_name() == "from_bytes"));
        assert!(!exports.iter().any(|e| e.short_name() == "bind"));
        assert!(!exports.iter().any(|e| e.short_name() == "listen"));
        assert!(vm.resolves_use(&["io".into(), "net".into(), "tcp".into()], "*"));
    }

    #[test]
    fn resolves_use_detects_virtual_paths() {
        let vm = VirtualModules::new();
        assert!(vm.resolves_use(&["prelude".into(), "ops".into()], "Eq"));
        assert!(vm.resolves_use(&["ffi".into(), "types".into()], "*"));
        assert!(!vm.resolves_use(&["foo".into()], "sadge"));
    }

    #[test]
    fn resolves_time_fs_env_crypto_exports() {
        let vm = VirtualModules::new();
        #[cfg(feature = "time")]
        assert!(vm.resolves_use(&["time".into()], "*"));
        assert!(vm.resolves_use(&["io".into(), "fs".into()], "*"));
        assert!(vm.resolves_use(&["env".into()], "*"));
        #[cfg(feature = "crypto")]
        assert!(vm.resolves_use(&["crypto".into()], "*"));

        #[cfg(feature = "time")]
        assert!(matches!(
            vm.resolve_item(&["time".into()], "epoch"),
            Some(BuiltinExport::HostFn {
                surface: "epoch",
                registry: "time_epoch"
            })
        ));
        assert!(matches!(
            vm.resolve_item(&["io".into(), "fs".into()], "exists"),
            Some(BuiltinExport::HostFn {
                surface: "exists",
                registry: "fs_exists"
            })
        ));
        assert!(matches!(
            vm.resolve_item(&["env".into()], "var"),
            Some(BuiltinExport::HostFn {
                surface: "var",
                registry: "env_var"
            })
        ));
        #[cfg(feature = "crypto")]
        assert!(matches!(
            vm.resolve_item(&["crypto".into()], "sha256"),
            Some(BuiltinExport::HostFn {
                surface: "sha256",
                registry: "crypto_sha256"
            })
        ));
        #[cfg(feature = "regex")]
        assert!(vm.resolves_use(&["regex".into()], "*"));
        #[cfg(feature = "regex")]
        assert!(matches!(
            vm.resolve_item(&["regex".into()], "compile"),
            Some(BuiltinExport::HostFn {
                surface: "compile",
                registry: "regex_compile"
            })
        ));
    }
}
