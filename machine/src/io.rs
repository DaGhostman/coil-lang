//! Host-backed non-blocking IO streams (files, stdio, TCP).
//!
//! Streams are always non-blocking at the OS level. Sync helpers
//! (`read_exact`, `read_to_end`, `write_all`, …) may block in Rust
//! via `poll`, but never busy-spin on `WouldBlock`.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::time::Duration;

use common::{
    BUILTIN_IO_ERROR_VARIANTS, BUILTIN_OPTION_VARIANTS, BUILTIN_RESULT_VARIANTS, Value,
};

use crate::memory::{Heap, Member, ObjArray, ObjEnum, ObjStream, ObjTuple, Object, StreamKind};

/// Tag indices for [`IoError`](common::BUILTIN_IO_ERROR_ENUM).
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IoErrorTag {
    WouldBlock = 0,
    NotFound = 1,
    PermissionDenied = 2,
    AlreadyClosed = 3,
    InvalidInput = 4,
    Other = 5,
}

impl IoErrorTag {
    pub fn from_kind(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => Self::WouldBlock,
            ErrorKind::NotFound => Self::NotFound,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::InvalidInput => Self::InvalidInput,
            _ => Self::Other,
        }
    }
}

/// Allocate `Result::Ok(payload)` on the heap.
pub fn alloc_result_ok(heap: &mut Heap, payload: Value) -> Value {
    let _ = BUILTIN_RESULT_VARIANTS;
    alloc_enum(heap, 0, vec![member_from_value(heap, payload)])
}

/// Allocate `Result::Err(payload)` on the heap.
pub fn alloc_result_err(heap: &mut Heap, payload: Value) -> Value {
    alloc_enum(heap, 1, vec![member_from_value(heap, payload)])
}

/// Allocate `Option::None`.
pub fn alloc_option_none(heap: &mut Heap) -> Value {
    let _ = BUILTIN_OPTION_VARIANTS;
    alloc_enum(heap, 0, vec![])
}

/// Allocate `Option::Some(payload)`.
pub fn alloc_option_some(heap: &mut Heap, payload: Value) -> Value {
    alloc_enum(heap, 1, vec![member_from_value(heap, payload)])
}

/// Allocate a unit-payload `IoError` variant.
pub fn alloc_io_error(heap: &mut Heap, tag: IoErrorTag) -> Value {
    let _ = BUILTIN_IO_ERROR_VARIANTS;
    alloc_enum(heap, tag as u32, vec![])
}

fn alloc_enum(heap: &mut Heap, tag: u32, payload: Vec<Member>) -> Value {
    let (obj, _) = heap.alloc(ObjEnum { tag, payload }, Object::Enum);
    Value::from(obj.addr())
}

fn member_from_value(heap: &Heap, value: Value) -> Member {
    if !value.raw().is_null()
        && let Some(obj) = heap.find_object_by_addr(value.raw() as u64)
    {
        Member::Object(obj)
    } else {
        Member::Value(value)
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn poll_fd(fd: RawFd, for_read: bool, timeout: Option<Duration>) -> io::Result<bool> {
    let mut pfd = libc::pollfd {
        fd,
        events: if for_read {
            libc::POLLIN
        } else {
            libc::POLLOUT
        },
        revents: 0,
    };
    let timeout_ms = match timeout {
        None => -1,
        Some(d) => d.as_millis().min(i32::MAX as u128) as i32,
    };
    loop {
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        return Ok(rc > 0);
    }
}

/// Wrap an owned fd as a heap `Stream` (always non-blocking).
pub fn alloc_stream(heap: &mut Heap, fd: OwnedFd, kind: StreamKind) -> io::Result<Value> {
    set_nonblocking(fd.as_raw_fd())?;
    let (obj, _) = heap.alloc(
        ObjStream {
            fd: Some(fd),
            kind,
            closed: false,
        },
        Object::Stream,
    );
    Ok(Value::from(obj.addr()))
}

pub fn stream_stdin(heap: &mut Heap) -> Result<Value, IoErrorTag> {
    // Dup so closing the Stream does not close process stdin.
    let raw = unsafe { libc::dup(libc::STDIN_FILENO) };
    if raw < 0 {
        return Err(IoErrorTag::from_kind(io::Error::last_os_error().kind()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    alloc_stream(heap, fd, StreamKind::Stdin).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

pub fn stream_stdout(heap: &mut Heap) -> Result<Value, IoErrorTag> {
    let raw = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if raw < 0 {
        return Err(IoErrorTag::from_kind(io::Error::last_os_error().kind()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    alloc_stream(heap, fd, StreamKind::Stdout).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

pub fn stream_stderr(heap: &mut Heap) -> Result<Value, IoErrorTag> {
    let raw = unsafe { libc::dup(libc::STDERR_FILENO) };
    if raw < 0 {
        return Err(IoErrorTag::from_kind(io::Error::last_os_error().kind()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    alloc_stream(heap, fd, StreamKind::Stderr).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

fn open_flags(mode: &str) -> Result<i32, IoErrorTag> {
    match mode {
        "r" => Ok(libc::O_RDONLY),
        "w" => Ok(libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC),
        "a" => Ok(libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND),
        "rw" => Ok(libc::O_RDWR | libc::O_CREAT),
        _ => Err(IoErrorTag::InvalidInput),
    }
}

pub fn stream_open(heap: &mut Heap, path: &str, mode: &str) -> Result<Value, IoErrorTag> {
    let flags = open_flags(mode)? | libc::O_NONBLOCK;
    let c_path = std::ffi::CString::new(path).map_err(|_| IoErrorTag::InvalidInput)?;
    let raw = unsafe { libc::open(c_path.as_ptr(), flags, 0o666) };
    if raw < 0 {
        return Err(IoErrorTag::from_kind(io::Error::last_os_error().kind()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    alloc_stream(heap, fd, StreamKind::File).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

fn with_stream_mut<R>(
    heap: &mut Heap,
    stream: Value,
    f: impl FnOnce(&mut ObjStream) -> R,
) -> Result<R, IoErrorTag> {
    let addr = stream.raw() as u64;
    let Some(Object::Stream(mut gc)) = heap.find_object_by_addr(addr) else {
        return Err(IoErrorTag::InvalidInput);
    };
    Ok(f(gc.as_mut()))
}

pub fn stream_close(heap: &mut Heap, stream: Value) -> Result<(), IoErrorTag> {
    with_stream_mut(heap, stream, |s| {
        if s.closed {
            return Err(IoErrorTag::AlreadyClosed);
        }
        s.fd.take();
        s.closed = true;
        Ok(())
    })?
}

/// Non-blocking read into an existing `[byte]` array. Returns:
/// - `Ok(Some(n))` bytes written into the buffer
/// - `Ok(None)` EOF
/// - `Err(WouldBlock)` / other
pub fn stream_read(
    heap: &mut Heap,
    stream: Value,
    buf: Value,
) -> Result<Option<usize>, IoErrorTag> {
    let buf_addr = buf.raw() as u64;
    let capacity = match heap.find_object_by_addr(buf_addr) {
        Some(Object::Array(arr_gc)) => arr_gc.as_ref().elements.len(),
        _ => return Err(IoErrorTag::InvalidInput),
    };
    if capacity == 0 {
        return Ok(Some(0));
    }
    let mut tmp = vec![0u8; capacity];

    let n = with_stream_mut(heap, stream, |s| -> Result<Option<usize>, IoErrorTag> {
        if s.closed || s.fd.is_none() {
            return Err(IoErrorTag::AlreadyClosed);
        }
        let fd = s.fd.as_ref().unwrap().as_raw_fd();
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let result = match file.read(&mut tmp) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(n)),
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
                Err(IoErrorTag::WouldBlock)
            }
            Err(e) => Err(IoErrorTag::from_kind(e.kind())),
        };
        // Don't close the fd when `file` drops.
        let _ = file.into_raw_fd();
        result
    })??;

    if let Some(n) = n {
        let Some(Object::Array(mut arr_gc)) = heap.find_object_by_addr(buf_addr) else {
            return Err(IoErrorTag::InvalidInput);
        };
        let arr: &mut ObjArray = arr_gc.as_mut();
        for i in 0..n {
            arr.elements[i] = Value::from(tmp[i] as i64);
        }
        Ok(Some(n))
    } else {
        Ok(None)
    }
}

pub fn stream_write(heap: &mut Heap, stream: Value, buf: Value) -> Result<usize, IoErrorTag> {
    let buf_addr = buf.raw() as u64;
    let bytes: Vec<u8> = match heap.find_object_by_addr(buf_addr) {
        Some(Object::Array(arr_gc)) => arr_gc
            .as_ref()
            .elements
            .iter()
            .map(|v| {
                let n = v.as_int();
                if !(0..=255).contains(&n) {
                    0
                } else {
                    n as u8
                }
            })
            .collect(),
        _ => return Err(IoErrorTag::InvalidInput),
    };

    with_stream_mut(heap, stream, |s| -> Result<usize, IoErrorTag> {
        if s.closed || s.fd.is_none() {
            return Err(IoErrorTag::AlreadyClosed);
        }
        let fd = s.fd.as_ref().unwrap().as_raw_fd();
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let result = match file.write(&bytes) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
                Err(IoErrorTag::WouldBlock)
            }
            Err(e) => Err(IoErrorTag::from_kind(e.kind())),
        };
        let _ = file.into_raw_fd();
        result
    })?
}

/// Block until `buf.len()` bytes are read, EOF, or a hard error.
pub fn stream_read_exact(
    heap: &mut Heap,
    stream: Value,
    buf: Value,
) -> Result<Option<usize>, IoErrorTag> {
    let buf_addr = buf.raw() as u64;
    let Some(Object::Array(arr_gc)) = heap.find_object_by_addr(buf_addr) else {
        return Err(IoErrorTag::InvalidInput);
    };
    let need = arr_gc.as_ref().elements.len();
    let mut filled = 0usize;
    while filled < need {
        // Read into a temporary array view by slicing conceptually —
        // we read into the remaining suffix via a scratch then copy.
        let remaining = need - filled;
        let scratch_vals: Vec<Value> = (0..remaining).map(|_| Value::from(0_i64)).collect();
        let (scratch_obj, _) = heap.alloc(
            ObjArray {
                elements: scratch_vals,
            },
            Object::Array,
        );
        let scratch = Value::from(scratch_obj.addr());
        match stream_read(heap, stream, scratch) {
            Ok(None) => {
                return if filled == 0 {
                    Ok(None)
                } else {
                    Ok(Some(filled))
                };
            }
            Ok(Some(0)) => {
                // Spurious; wait for readability.
                wait_readable(heap, stream)?;
            }
            Ok(Some(n)) => {
                let Some(Object::Array(src)) = heap.find_object_by_addr(scratch.raw() as u64)
                else {
                    return Err(IoErrorTag::Other);
                };
                let chunk: Vec<Value> = src.as_ref().elements[..n].to_vec();
                let Some(Object::Array(mut dst)) = heap.find_object_by_addr(buf_addr) else {
                    return Err(IoErrorTag::Other);
                };
                for (i, v) in chunk.into_iter().enumerate() {
                    dst.as_mut().elements[filled + i] = v;
                }
                filled += n;
            }
            Err(IoErrorTag::WouldBlock) => {
                wait_readable(heap, stream)?;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Some(filled))
}

/// Block until EOF; return a new `[byte]` with all data.
pub fn stream_read_to_end(heap: &mut Heap, stream: Value) -> Result<Value, IoErrorTag> {
    let mut acc: Vec<u8> = Vec::new();
    let chunk_size = 4096usize;
    loop {
        let scratch_vals: Vec<Value> = (0..chunk_size).map(|_| Value::from(0_i64)).collect();
        let (scratch_obj, _) = heap.alloc(
            ObjArray {
                elements: scratch_vals,
            },
            Object::Array,
        );
        let scratch = Value::from(scratch_obj.addr());
        match stream_read(heap, stream, scratch) {
            Ok(None) => break,
            Ok(Some(0)) => wait_readable(heap, stream)?,
            Ok(Some(n)) => {
                let Some(Object::Array(src)) = heap.find_object_by_addr(scratch.raw() as u64)
                else {
                    return Err(IoErrorTag::Other);
                };
                for v in &src.as_ref().elements[..n] {
                    acc.push(v.as_int() as u8);
                }
            }
            Err(IoErrorTag::WouldBlock) => wait_readable(heap, stream)?,
            Err(e) => return Err(e),
        }
    }
    let elements: Vec<Value> = acc.iter().map(|&b| Value::from(b as i64)).collect();
    let (obj, _) = heap.alloc(ObjArray { elements }, Object::Array);
    Ok(Value::from(obj.addr()))
}

/// Block until the entire buffer is written.
pub fn stream_write_all(heap: &mut Heap, stream: Value, buf: Value) -> Result<(), IoErrorTag> {
    let buf_addr = buf.raw() as u64;
    let Some(Object::Array(arr_gc)) = heap.find_object_by_addr(buf_addr) else {
        return Err(IoErrorTag::InvalidInput);
    };
    let mut bytes: Vec<u8> = arr_gc
        .as_ref()
        .elements
        .iter()
        .map(|v| v.as_int() as u8)
        .collect();
    let mut offset = 0usize;
    while offset < bytes.len() {
        // Write remaining suffix via a temp array.
        let rest: Vec<Value> = bytes[offset..]
            .iter()
            .map(|&b| Value::from(b as i64))
            .collect();
        let (tmp_obj, _) = heap.alloc(ObjArray { elements: rest }, Object::Array);
        let tmp = Value::from(tmp_obj.addr());
        match stream_write(heap, stream, tmp) {
            Ok(0) => wait_writable(heap, stream)?,
            Ok(n) => offset += n,
            Err(IoErrorTag::WouldBlock) => wait_writable(heap, stream)?,
            Err(e) => return Err(e),
        }
        let _ = &mut bytes; // silence
    }
    Ok(())
}

fn wait_readable(heap: &mut Heap, stream: Value) -> Result<(), IoErrorTag> {
    let fd = stream_raw_fd(heap, stream)?;
    poll_fd(fd, true, None).map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    Ok(())
}

fn wait_writable(heap: &mut Heap, stream: Value) -> Result<(), IoErrorTag> {
    let fd = stream_raw_fd(heap, stream)?;
    poll_fd(fd, false, None).map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    Ok(())
}

fn stream_raw_fd(heap: &mut Heap, stream: Value) -> Result<RawFd, IoErrorTag> {
    with_stream_mut(heap, stream, |s| {
        if s.closed || s.fd.is_none() {
            Err(IoErrorTag::AlreadyClosed)
        } else {
            Ok(s.fd.as_ref().unwrap().as_raw_fd())
        }
    })?
}

// ---- TCP ----

pub fn tcp_connect(heap: &mut Heap, host: &str, port: i64) -> Result<Value, IoErrorTag> {
    if !(0..=65535).contains(&port) {
        return Err(IoErrorTag::InvalidInput);
    }
    let addr = format!("{host}:{port}");
    // Blocking connect (sync adapter), then hand back a non-blocking stream.
    let stream = TcpStream::connect(&addr).map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    stream
        .set_nonblocking(true)
        .map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    let fd = stream.into_raw_fd();
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    alloc_stream(heap, owned, StreamKind::Tcp).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

pub fn tcp_listen(heap: &mut Heap, host: &str, port: i64) -> Result<Value, IoErrorTag> {
    if !(0..=65535).contains(&port) {
        return Err(IoErrorTag::InvalidInput);
    }
    let addr = format!("{host}:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    let fd = listener.into_raw_fd();
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    alloc_stream(heap, owned, StreamKind::TcpListener).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

/// Non-blocking accept. `WouldBlock` if nothing pending.
pub fn tcp_accept(heap: &mut Heap, listener: Value) -> Result<Value, IoErrorTag> {
    let fd = stream_raw_fd(heap, listener)?;
    // Reconstruct listener without taking ownership permanently.
    let listener = unsafe { TcpListener::from_raw_fd(fd) };
    let result = match listener.accept() {
        Ok((stream, _)) => {
            stream
                .set_nonblocking(true)
                .map_err(|e| IoErrorTag::from_kind(e.kind()))?;
            let raw = stream.into_raw_fd();
            let owned = unsafe { OwnedFd::from_raw_fd(raw) };
            alloc_stream(heap, owned, StreamKind::Tcp).map_err(|e| IoErrorTag::from_kind(e.kind()))
        }
        Err(e) if e.kind() == ErrorKind::WouldBlock => Err(IoErrorTag::WouldBlock),
        Err(e) => Err(IoErrorTag::from_kind(e.kind())),
    };
    // Don't close the listener fd.
    let _ = listener.into_raw_fd();
    result
}

/// Block until a connection is accepted.
pub fn tcp_accept_wait(heap: &mut Heap, listener: Value) -> Result<Value, IoErrorTag> {
    loop {
        match tcp_accept(heap, listener) {
            Err(IoErrorTag::WouldBlock) => wait_readable(heap, listener)?,
            other => return other,
        }
    }
}

// ---- UDP ----

fn alloc_tuple3(heap: &mut Heap, a: Value, b: Value, c: Value) -> Value {
    let (obj, _) = heap.alloc(
        ObjTuple {
            elements: vec![a, b, c],
        },
        Object::Tuple,
    );
    Value::from(obj.addr())
}

fn parse_socket_addr(host: &str, port: i64) -> Result<SocketAddr, IoErrorTag> {
    if !(0..=65535).contains(&port) {
        return Err(IoErrorTag::InvalidInput);
    }
    format!("{host}:{port}")
        .parse()
        .map_err(|_| IoErrorTag::InvalidInput)
}

/// Bind a non-blocking UDP socket. `port` may be `0` (ephemeral);
/// use [`udp_local_port`] to read the assigned port.
pub fn udp_bind(heap: &mut Heap, host: &str, port: i64) -> Result<Value, IoErrorTag> {
    let addr = parse_socket_addr(host, port)?;
    let sock = UdpSocket::bind(addr).map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    sock.set_nonblocking(true)
        .map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    let fd = sock.into_raw_fd();
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    alloc_stream(heap, owned, StreamKind::Udp).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

/// Create a connected non-blocking UDP socket toward `(host, port)`.
///
/// After connect, [`stream_read`] / [`stream_write`] (and the sync adapters)
/// work against that peer. Unconnected peers still use
/// [`udp_send_to`] / [`udp_recv_from`].
pub fn udp_connect(heap: &mut Heap, host: &str, port: i64) -> Result<Value, IoErrorTag> {
    let peer = parse_socket_addr(host, port)?;
    // Ephemeral local bind, then connect for a default peer.
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    sock.connect(peer)
        .map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    sock.set_nonblocking(true)
        .map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    let fd = sock.into_raw_fd();
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    alloc_stream(heap, owned, StreamKind::Udp).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

/// Local UDP port (after bind / connect).
pub fn udp_local_port(heap: &mut Heap, stream: Value) -> Result<i64, IoErrorTag> {
    let fd = stream_raw_fd(heap, stream)?;
    let sock = unsafe { UdpSocket::from_raw_fd(fd) };
    let result = sock
        .local_addr()
        .map(|a| a.port() as i64)
        .map_err(|e| IoErrorTag::from_kind(e.kind()));
    let _ = sock.into_raw_fd();
    result
}

/// Non-blocking `sendto`. Returns bytes sent.
pub fn udp_send_to(
    heap: &mut Heap,
    stream: Value,
    buf: Value,
    host: &str,
    port: i64,
) -> Result<usize, IoErrorTag> {
    let peer = parse_socket_addr(host, port)?;
    let bytes = value_as_bytes(heap, buf)?;
    let fd = stream_raw_fd(heap, stream)?;
    let sock = unsafe { UdpSocket::from_raw_fd(fd) };
    let result = match sock.send_to(&bytes, peer) {
        Ok(n) => Ok(n),
        Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
            Err(IoErrorTag::WouldBlock)
        }
        Err(e) => Err(IoErrorTag::from_kind(e.kind())),
    };
    let _ = sock.into_raw_fd();
    result
}

/// Non-blocking `recvfrom` into `buf`.
///
/// On success returns a heap tuple `(nbytes: int, peer_host: string, peer_port: int)`.
/// The first `nbytes` elements of `buf` are filled.
pub fn udp_recv_from(heap: &mut Heap, stream: Value, buf: Value) -> Result<Value, IoErrorTag> {
    let buf_addr = buf.raw() as u64;
    let capacity = match heap.find_object_by_addr(buf_addr) {
        Some(Object::Array(arr_gc)) => arr_gc.as_ref().elements.len(),
        _ => return Err(IoErrorTag::InvalidInput),
    };
    if capacity == 0 {
        let host = {
            let gc = heap.intern(String::new());
            Value::from(gc.as_ptr() as *mut u8 as u64)
        };
        return Ok(alloc_tuple3(
            heap,
            Value::from(0_i64),
            host,
            Value::from(0_i64),
        ));
    }
    let mut tmp = vec![0u8; capacity];

    let (n, peer) = {
        let fd = stream_raw_fd(heap, stream)?;
        let sock = unsafe { UdpSocket::from_raw_fd(fd) };
        let result = match sock.recv_from(&mut tmp) {
            Ok((n, peer)) => Ok((n, peer)),
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
                Err(IoErrorTag::WouldBlock)
            }
            Err(e) => Err(IoErrorTag::from_kind(e.kind())),
        };
        let _ = sock.into_raw_fd();
        result?
    };

    {
        let Some(Object::Array(mut arr_gc)) = heap.find_object_by_addr(buf_addr) else {
            return Err(IoErrorTag::InvalidInput);
        };
        let arr: &mut ObjArray = arr_gc.as_mut();
        for i in 0..n {
            arr.elements[i] = Value::from(tmp[i] as i64);
        }
    }

    let host_str = match peer {
        SocketAddr::V4(a) => a.ip().to_string(),
        SocketAddr::V6(a) => a.ip().to_string(),
    };
    let host = {
        let gc = heap.intern(host_str);
        Value::from(gc.as_ptr() as *mut u8 as u64)
    };
    Ok(alloc_tuple3(
        heap,
        Value::from(n as i64),
        host,
        Value::from(peer.port() as i64),
    ))
}

/// Block until a datagram arrives, then [`udp_recv_from`].
pub fn udp_recv_from_wait(heap: &mut Heap, stream: Value, buf: Value) -> Result<Value, IoErrorTag> {
    loop {
        match udp_recv_from(heap, stream, buf) {
            Err(IoErrorTag::WouldBlock) => wait_readable(heap, stream)?,
            other => return other,
        }
    }
}

/// Decode a heap string `Value` into a Rust `String`.
pub fn value_as_string(heap: &Heap, v: Value) -> Result<String, IoErrorTag> {
    match heap.find_object_by_addr(v.raw() as u64) {
        Some(Object::String(gc)) => Ok(gc.as_ref().data.clone()),
        _ => Err(IoErrorTag::InvalidInput),
    }
}

/// Read a heap `[byte]` array into a Rust `Vec<u8>`.
pub fn value_as_bytes(heap: &Heap, v: Value) -> Result<Vec<u8>, IoErrorTag> {
    match heap.find_object_by_addr(v.raw() as u64) {
        Some(Object::Array(arr_gc)) => Ok(arr_gc
            .as_ref()
            .elements
            .iter()
            .map(|e| {
                let n = e.as_int();
                if (0..=255).contains(&n) {
                    n as u8
                } else {
                    // Out-of-range elements are a typechecker bug; clamp
                    // defensively rather than panicking in the host.
                    (n as u8)
                }
            })
            .collect()),
        _ => Err(IoErrorTag::InvalidInput),
    }
}

/// Decode `[byte]` as UTF-8 into a heap string.
///
/// Invalid UTF-8 → `Err(InvalidInput)`.
pub fn from_bytes(heap: &mut Heap, buf: Value) -> Result<Value, IoErrorTag> {
    let bytes = value_as_bytes(heap, buf)?;
    let s = String::from_utf8(bytes).map_err(|_| IoErrorTag::InvalidInput)?;
    let gc = heap.intern(s);
    Ok(Value::from(gc.as_ptr() as *mut u8 as u64))
}

/// Encode a heap string as a fresh `[byte]` array (UTF-8).
///
/// Non-string input yields an empty array (defensive — the typechecker
/// rejects this case).
pub fn to_bytes(heap: &mut Heap, s: Value) -> Value {
    let bytes = match value_as_string(heap, s) {
        Ok(text) => text.into_bytes(),
        Err(_) => Vec::new(),
    };
    let elements: Vec<Value> = bytes.iter().map(|&b| Value::from(b as i64)).collect();
    let (obj, _) = heap.alloc(ObjArray { elements }, Object::Array);
    Value::from(obj.addr())
}

/// Helper: wrap a fallible stream op that returns a Value into `Result<_, IoError>`.
pub fn as_result_value(heap: &mut Heap, r: Result<Value, IoErrorTag>) -> Value {
    match r {
        Ok(v) => alloc_result_ok(heap, v),
        Err(tag) => {
            let err = alloc_io_error(heap, tag);
            alloc_result_err(heap, err)
        }
    }
}

/// Helper: `Result<Option<int>, IoError>` encoding.
pub fn as_result_option_int(heap: &mut Heap, r: Result<Option<usize>, IoErrorTag>) -> Value {
    match r {
        Ok(None) => {
            let none = alloc_option_none(heap);
            alloc_result_ok(heap, none)
        }
        Ok(Some(n)) => {
            let some = alloc_option_some(heap, Value::from(n as i64));
            alloc_result_ok(heap, some)
        }
        Err(tag) => {
            let err = alloc_io_error(heap, tag);
            alloc_result_err(heap, err)
        }
    }
}

/// Helper: `Result<int, IoError>`.
pub fn as_result_int(heap: &mut Heap, r: Result<usize, IoErrorTag>) -> Value {
    match r {
        Ok(n) => alloc_result_ok(heap, Value::from(n as i64)),
        Err(tag) => {
            let err = alloc_io_error(heap, tag);
            alloc_result_err(heap, err)
        }
    }
}

/// Helper: `Result<(), IoError>` — Ok payload is unit (null/default).
pub fn as_result_unit(heap: &mut Heap, r: Result<(), IoErrorTag>) -> Value {
    match r {
        Ok(()) => alloc_result_ok(heap, Value::default()),
        Err(tag) => {
            let err = alloc_io_error(heap, tag);
            alloc_result_err(heap, err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Heap;
    use std::io::{Read as IoRead, Write as IoWrite};
    use std::net::{TcpListener, TcpStream};
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    use std::thread;
    use std::time::Duration;

    fn enum_tag(heap: &Heap, v: Value) -> Option<u32> {
        match heap.find_object_by_addr(v.raw() as u64) {
            Some(Object::Enum(gc)) => Some(gc.as_ref().tag),
            _ => None,
        }
    }

    fn make_byte_array(heap: &mut Heap, bytes: &[u8]) -> Value {
        let elements: Vec<Value> = bytes.iter().map(|&b| Value::from(b as i64)).collect();
        let (obj, _) = heap.alloc(ObjArray { elements }, Object::Array);
        Value::from(obj.addr())
    }

    fn array_bytes(heap: &Heap, v: Value) -> Vec<u8> {
        match heap.find_object_by_addr(v.raw() as u64) {
            Some(Object::Array(gc)) => gc
                .as_ref()
                .elements
                .iter()
                .map(|e| e.as_int() as u8)
                .collect(),
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn file_round_trip_read_to_end() {
        let path = std::env::temp_dir().join("zero_script_io_unit_roundtrip.bin");
        let mut heap = Heap::default();
        let data = make_byte_array(&mut heap, b"Hi");
        let w = stream_open(&mut heap, path.to_str().unwrap(), "w").expect("open w");
        stream_write_all(&mut heap, w, data).expect("write_all");
        stream_close(&mut heap, w).expect("close w");

        let r = stream_open(&mut heap, path.to_str().unwrap(), "r").expect("open r");
        let buf = stream_read_to_end(&mut heap, r).expect("read_to_end");
        stream_close(&mut heap, r).expect("close r");
        assert_eq!(array_bytes(&heap, buf), b"Hi");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_file_read_returns_eof_none() {
        let path = std::env::temp_dir().join("zero_script_io_unit_eof.bin");
        {
            let _f = std::fs::File::create(&path).unwrap();
        }
        let mut heap = Heap::default();
        let s = stream_open(&mut heap, path.to_str().unwrap(), "r").expect("open");
        let buf = make_byte_array(&mut heap, &[0, 0, 0, 0]);
        let r = stream_read(&mut heap, s, buf).expect("read");
        assert_eq!(r, None);
        stream_close(&mut heap, s).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn close_then_read_is_already_closed() {
        let path = std::env::temp_dir().join("zero_script_io_unit_closed.bin");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"x").unwrap();
        }
        let mut heap = Heap::default();
        let s = stream_open(&mut heap, path.to_str().unwrap(), "r").unwrap();
        stream_close(&mut heap, s).unwrap();
        let buf = make_byte_array(&mut heap, &[0]);
        let err = stream_read(&mut heap, s, buf).unwrap_err();
        assert_eq!(err, IoErrorTag::AlreadyClosed);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn result_helpers_encode_ok_none_and_would_block() {
        let mut heap = Heap::default();
        let ok_none = as_result_option_int(&mut heap, Ok(None));
        assert_eq!(enum_tag(&heap, ok_none), Some(0));
        let err = as_result_option_int(&mut heap, Err(IoErrorTag::WouldBlock));
        assert_eq!(enum_tag(&heap, err), Some(1));
    }

    #[test]
    fn from_bytes_decodes_utf8() {
        let mut heap = Heap::default();
        let buf = make_byte_array(&mut heap, b"hello");
        let s = from_bytes(&mut heap, buf).expect("utf-8");
        assert_eq!(value_as_string(&heap, s).unwrap(), "hello");
    }

    #[test]
    fn from_bytes_rejects_invalid_utf8() {
        let mut heap = Heap::default();
        let buf = make_byte_array(&mut heap, &[0xff, 0xfe]);
        let err = from_bytes(&mut heap, buf).unwrap_err();
        assert_eq!(err, IoErrorTag::InvalidInput);
    }

    #[test]
    fn to_bytes_round_trips_with_from_bytes() {
        let mut heap = Heap::default();
        let s = {
            let gc = heap.intern("Hi".into());
            Value::from(gc.as_ptr() as *mut u8 as u64)
        };
        let buf = to_bytes(&mut heap, s);
        assert_eq!(array_bytes(&heap, buf), b"Hi");
        let back = from_bytes(&mut heap, buf).expect("round-trip");
        assert_eq!(value_as_string(&heap, back).unwrap(), "Hi");
    }

    #[test]
    fn from_bytes_as_result_wraps_ok_and_err() {
        let mut heap = Heap::default();
        let ok_buf = make_byte_array(&mut heap, b"x");
        let ok_inner = from_bytes(&mut heap, ok_buf);
        let ok = as_result_value(&mut heap, ok_inner);
        assert_eq!(enum_tag(&heap, ok), Some(0));

        let err_buf = make_byte_array(&mut heap, &[0x80]);
        let err_inner = from_bytes(&mut heap, err_buf);
        let err = as_result_value(&mut heap, err_inner);
        assert_eq!(enum_tag(&heap, err), Some(1));
    }

    fn tuple_elems(heap: &Heap, v: Value) -> Vec<Value> {
        match heap.find_object_by_addr(v.raw() as u64) {
            Some(Object::Tuple(gc)) => gc.as_ref().elements.clone(),
            _ => panic!("expected tuple"),
        }
    }

    #[test]
    fn udp_bind_send_to_recv_from_round_trip() {
        let mut heap = Heap::default();
        let server = udp_bind(&mut heap, "127.0.0.1", 0).expect("bind server");
        let port = udp_local_port(&mut heap, server).expect("local port");
        assert!(port > 0);

        let client = udp_bind(&mut heap, "127.0.0.1", 0).expect("bind client");
        let msg = make_byte_array(&mut heap, b"Hi");
        let n = udp_send_to(&mut heap, client, msg, "127.0.0.1", port).expect("send_to");
        assert_eq!(n, 2);

        let buf = make_byte_array(&mut heap, &[0, 0, 0, 0, 0, 0, 0, 0]);
        let t = udp_recv_from_wait(&mut heap, server, buf).expect("recv");
        let elems = tuple_elems(&heap, t);
        assert_eq!(elems[0].as_int(), 2);
        assert_eq!(elems[2].as_int(), udp_local_port(&mut heap, client).unwrap());
        assert_eq!(&array_bytes(&heap, buf)[..2], b"Hi");

        stream_close(&mut heap, server).unwrap();
        stream_close(&mut heap, client).unwrap();
    }

    #[test]
    fn udp_connect_write_read_round_trip() {
        let mut heap = Heap::default();
        let server = udp_bind(&mut heap, "127.0.0.1", 0).expect("bind server");
        let port = udp_local_port(&mut heap, server).expect("port");
        let client = udp_connect(&mut heap, "127.0.0.1", port).expect("connect");

        let msg = make_byte_array(&mut heap, b"Yo");
        stream_write_all(&mut heap, client, msg).expect("write_all");

        let buf = make_byte_array(&mut heap, &[0, 0, 0, 0]);
        let t = udp_recv_from_wait(&mut heap, server, buf).expect("recv");
        assert_eq!(tuple_elems(&heap, t)[0].as_int(), 2);
        assert_eq!(&array_bytes(&heap, buf)[..2], b"Yo");

        stream_close(&mut heap, server).unwrap();
        stream_close(&mut heap, client).unwrap();
    }

    #[test]
    fn tcp_listen_accept_echo_localhost() {
        let mut heap = Heap::default();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port() as u16;
        listener.set_nonblocking(true).unwrap();
        let fd = listener.into_raw_fd();
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let listen_stream = alloc_stream(&mut heap, owned, StreamKind::TcpListener).unwrap();

        let client = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"ping").unwrap();
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).unwrap();
            assert_eq!(&buf, b"ping");
        });

        let conn = tcp_accept_wait(&mut heap, listen_stream).expect("accept");
        let buf = make_byte_array(&mut heap, &[0, 0, 0, 0]);
        let n = stream_read_exact(&mut heap, conn, buf).expect("read_exact");
        assert_eq!(n, Some(4));
        assert_eq!(&array_bytes(&heap, buf)[..4], b"ping");
        let reply = make_byte_array(&mut heap, b"ping");
        stream_write_all(&mut heap, conn, reply).unwrap();
        stream_close(&mut heap, conn).unwrap();
        client.join().unwrap();
        stream_close(&mut heap, listen_stream).unwrap();
    }
}
