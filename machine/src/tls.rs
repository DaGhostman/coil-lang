//! Host-backed TLS client streams via rustls (`io::net::tls`).
//!
//! Handshake completes in the host during [`tls_connect`] /
//! [`tls_connect_insecure`]; afterwards the handle is a normal non-blocking
//! [`crate::memory::ObjStream`] with [`StreamKind::Tls`](crate::memory::StreamKind::Tls).

use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::sync::{Arc, OnceLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as TlsError, RootCertStore,
    SignatureScheme,
};

use common::Value;

use crate::io::{alloc_tls_stream, IoErrorTag};
use crate::memory::Heap;

/// rustls client session state owned by a TLS [`crate::memory::ObjStream`].
pub struct TlsSession {
    pub(crate) conn: ClientConnection,
    /// Plaintext drained from rustls but not yet returned to coil.
    plaintext: Vec<u8>,
    plaintext_pos: usize,
}

impl TlsSession {
    fn new(conn: ClientConnection) -> Self {
        Self {
            conn,
            plaintext: Vec::new(),
            plaintext_pos: 0,
        }
    }

    /// True when app data is buffered and a read need not wait on the socket.
    pub fn has_buffered_plaintext(&self) -> bool {
        self.plaintext_pos < self.plaintext.len()
    }

    fn drain_plaintext_into(&mut self, out: &mut [u8]) -> usize {
        let avail = self.plaintext.len() - self.plaintext_pos;
        if avail == 0 || out.is_empty() {
            return 0;
        }
        let n = avail.min(out.len());
        out[..n].copy_from_slice(&self.plaintext[self.plaintext_pos..self.plaintext_pos + n]);
        self.plaintext_pos += n;
        if self.plaintext_pos >= self.plaintext.len() {
            self.plaintext.clear();
            self.plaintext_pos = 0;
        }
        n
    }

    fn pull_plaintext_from_conn(&mut self) -> Result<(), IoErrorTag> {
        loop {
            let mut tmp = [0u8; 16 * 1024];
            match self.conn.reader().read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => self.plaintext.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                Err(_) => return Err(IoErrorTag::Other),
            }
        }
        Ok(())
    }
}

fn map_tls_err(_e: TlsError) -> IoErrorTag {
    IoErrorTag::Other
}

fn map_io(e: io::Error) -> IoErrorTag {
    match e.kind() {
        ErrorKind::WouldBlock | ErrorKind::Interrupted => IoErrorTag::WouldBlock,
        other => IoErrorTag::from_kind(other),
    }
}

fn verified_config() -> Arc<ClientConfig> {
    static CFG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    })
    .clone()
}

fn insecure_config() -> Arc<ClientConfig> {
    static CFG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        Arc::new(
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoCertVerify))
                .with_no_client_auth(),
        )
    })
    .clone()
}

/// Dangerous verifier for `connect_insecure`: skips **trust** / name checks only.
/// TLS 1.2/1.3 record signatures are still verified via the default provider.
#[derive(Debug)]
struct NoCertVerify;

impl ServerCertVerifier for NoCertVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn parse_server_name(host: &str) -> Result<ServerName<'static>, IoErrorTag> {
    ServerName::try_from(host.to_string()).map_err(|_| IoErrorTag::InvalidInput)
}

fn handshake_blocking(stream: &mut TcpStream, conn: &mut ClientConnection) -> Result<(), IoErrorTag> {
    while conn.is_handshaking() {
        while conn.wants_write() {
            conn.write_tls(stream).map_err(map_io)?;
        }
        if conn.is_handshaking() {
            match conn.read_tls(stream) {
                Ok(0) => return Err(IoErrorTag::Other),
                Ok(_) => {
                    let _ = conn.process_new_packets().map_err(map_tls_err)?;
                }
                // Socket is still blocking here; WouldBlock/Interrupted are unexpected.
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
                    std::thread::yield_now();
                    continue;
                }
                Err(e) => return Err(map_io(e)),
            }
        }
    }
    // Flush any post-handshake tickets / CCS still pending.
    while conn.wants_write() {
        let _ = conn.write_tls(stream).map_err(map_io)?;
    }
    Ok(())
}

fn connect_with_config(
    heap: &mut Heap,
    host: &str,
    port: i64,
    config: Arc<ClientConfig>,
) -> Result<Value, IoErrorTag> {
    if !(0..=65535).contains(&port) {
        return Err(IoErrorTag::InvalidInput);
    }
    let server_name = parse_server_name(host)?;
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    let mut conn = ClientConnection::new(config, server_name).map_err(map_tls_err)?;
    handshake_blocking(&mut stream, &mut conn)?;
    stream
        .set_nonblocking(true)
        .map_err(|e| IoErrorTag::from_kind(e.kind()))?;
    let fd = stream.into_raw_fd();
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    alloc_tls_stream(heap, owned, TlsSession::new(conn)).map_err(|e| IoErrorTag::from_kind(e.kind()))
}

/// TLS connect with webpki-roots verification + SNI from `host`.
pub fn tls_connect(heap: &mut Heap, host: &str, port: i64) -> Result<Value, IoErrorTag> {
    connect_with_config(heap, host, port, verified_config())
}

/// TLS connect that **disables certificate verification** (local / self-signed only).
///
/// Still performs a real TLS handshake; only the trust store / name checks are
/// skipped. Prefer [`tls_connect`] for anything beyond local development.
pub fn tls_connect_insecure(heap: &mut Heap, host: &str, port: i64) -> Result<Value, IoErrorTag> {
    connect_with_config(heap, host, port, insecure_config())
}

fn with_socket_mut<R>(fd: RawFd, f: impl FnOnce(&mut std::fs::File) -> R) -> R {
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let result = f(&mut file);
    let _ = file.into_raw_fd();
    result
}

fn flush_tls(fd: RawFd, tls: &mut TlsSession) -> Result<(), IoErrorTag> {
    while tls.conn.wants_write() {
        let n = with_socket_mut(fd, |sock| tls.conn.write_tls(sock)).map_err(map_io)?;
        if n == 0 {
            return Err(IoErrorTag::WouldBlock);
        }
    }
    Ok(())
}

fn read_tls_records(fd: RawFd, tls: &mut TlsSession) -> Result<usize, IoErrorTag> {
    match with_socket_mut(fd, |sock| tls.conn.read_tls(sock)) {
        Ok(0) => Ok(0),
        Ok(n) => {
            let _ = tls.conn.process_new_packets().map_err(map_tls_err)?;
            tls.pull_plaintext_from_conn()?;
            Ok(n)
        }
        Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
            Err(IoErrorTag::WouldBlock)
        }
        Err(e) => Err(map_io(e)),
    }
}

/// Non-blocking TLS application read into `buf`.
///
/// `Ok(None)` = clean EOF; `Err(WouldBlock)` when more socket data is needed.
pub fn tls_read(fd: RawFd, tls: &mut TlsSession, buf: &mut [u8]) -> Result<Option<usize>, IoErrorTag> {
    if buf.is_empty() {
        return Ok(Some(0));
    }
    // Drain pending ciphertext first so a prior write that returned Ok(n) with
    // a WouldBlock flush cannot leave the peer waiting while we poll for read.
    flush_tls(fd, tls)?;
    // Prefer already-buffered plaintext.
    let n = tls.drain_plaintext_into(buf);
    if n > 0 {
        return Ok(Some(n));
    }
    // Pull any plaintext already sitting in rustls.
    tls.pull_plaintext_from_conn()?;
    let n = tls.drain_plaintext_into(buf);
    if n > 0 {
        return Ok(Some(n));
    }
    // Need more TLS records from the socket.
    match read_tls_records(fd, tls) {
        Ok(0) => {
            // Peer closed; drain any final plaintext.
            tls.pull_plaintext_from_conn()?;
            let n = tls.drain_plaintext_into(buf);
            if n > 0 {
                Ok(Some(n))
            } else {
                Ok(None)
            }
        }
        Ok(_) => {
            let n = tls.drain_plaintext_into(buf);
            if n > 0 {
                Ok(Some(n))
            } else {
                // Record processed but no app data yet (e.g. key update).
                Err(IoErrorTag::WouldBlock)
            }
        }
        Err(e) => Err(e),
    }
}

/// Non-blocking TLS application write of `bytes`.
pub fn tls_write(fd: RawFd, tls: &mut TlsSession, bytes: &[u8]) -> Result<usize, IoErrorTag> {
    // Always try to flush pending ciphertext first.
    flush_tls(fd, tls)?;
    if bytes.is_empty() {
        return Ok(0);
    }
    let n = match tls.conn.writer().write(bytes) {
        Ok(n) => n,
        Err(e) if e.kind() == ErrorKind::WouldBlock => {
            flush_tls(fd, tls)?;
            return Err(IoErrorTag::WouldBlock);
        }
        Err(_) => return Err(IoErrorTag::Other),
    };
    // Best-effort flush; WouldBlock after accepting app bytes is OK — next
    // write/read will resume flushing via wants_write.
    match flush_tls(fd, tls) {
        Ok(()) | Err(IoErrorTag::WouldBlock) => Ok(n),
        Err(e) => Err(e),
    }
}

/// Send TLS `close_notify` (best-effort) before the fd is closed.
pub fn send_close_notify(fd: RawFd, tls: &mut TlsSession) -> Result<(), IoErrorTag> {
    tls.conn.send_close_notify();
    match flush_tls(fd, tls) {
        Ok(()) | Err(IoErrorTag::WouldBlock) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{stream_close, stream_read_to_end, stream_write_all};
    use crate::memory::{Heap, ObjArray, Object};
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::{ServerConfig, ServerConnection};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

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

    fn test_server_config() -> (Arc<ServerConfig>, String) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("cert");
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()));
        let cert_der = CertificateDer::from(cert.cert);
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key)
            .expect("server config");
        (Arc::new(config), "localhost".into())
    }

    /// Echo server: read app data, echo it back, then close_notify.
    /// Socket IO is time-bounded so aborted clients cannot hang the suite.
    fn spawn_tls_echo_server() -> (u16, thread::JoinHandle<()>) {
        let (cfg, _name) = test_server_config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (ready_tx, ready_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            ready_tx.send(()).ok();
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = sock.set_write_timeout(Some(Duration::from_secs(2)));
            let Ok(mut conn) = ServerConnection::new(cfg) else {
                return;
            };
            // Handshake (best-effort; client may abort mid-way).
            while conn.is_handshaking() {
                if conn.wants_write()
                    && conn.write_tls(&mut sock).is_err()
                {
                    return;
                }
                if conn.is_handshaking() {
                    match conn.read_tls(&mut sock) {
                        Ok(0) => return,
                        Ok(_) => {
                            if conn.process_new_packets().is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
            }
            while conn.wants_write() {
                if conn.write_tls(&mut sock).is_err() {
                    return;
                }
            }
            let mut acc = Vec::new();
            let hard_deadline = std::time::Instant::now() + Duration::from_secs(2);
            let mut last_data = std::time::Instant::now();
            // Accumulate until peer goes idle after first byte (or EOF / hard deadline).
            // A single-shot read is not enough for multi-record client writes.
            while std::time::Instant::now() < hard_deadline {
                if !acc.is_empty() && last_data.elapsed() > Duration::from_millis(50) {
                    break;
                }
                match conn.read_tls(&mut sock) {
                    Ok(0) => break,
                    Ok(_) => {
                        if conn.process_new_packets().is_err() {
                            return;
                        }
                        let mut tmp = [0u8; 4096];
                        loop {
                            match conn.reader().read(&mut tmp) {
                                Ok(0) => break,
                                Ok(n) => {
                                    acc.extend_from_slice(&tmp[..n]);
                                    last_data = std::time::Instant::now();
                                }
                                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                                Err(_) => break,
                            }
                        }
                    }
                    Err(e)
                        if e.kind() == ErrorKind::WouldBlock
                            || e.kind() == ErrorKind::TimedOut =>
                    {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
            if acc.is_empty() {
                return;
            }
            if conn.writer().write_all(&acc).is_err() {
                return;
            }
            conn.send_close_notify();
            while conn.wants_write() {
                if conn.write_tls(&mut sock).is_err() {
                    break;
                }
            }
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).expect("server ready");
        (port, handle)
    }

    #[test]
    fn connect_insecure_round_trips_bytes() {
        let (port, handle) = spawn_tls_echo_server();
        let mut heap = Heap::default();
        let s = tls_connect_insecure(&mut heap, "127.0.0.1", port as i64).expect("connect");
        let msg = make_byte_array(&mut heap, b"hello-tls");
        stream_write_all(&mut heap, s, msg).expect("write_all");
        let echoed = stream_read_to_end(&mut heap, s).expect("read_to_end");
        assert_eq!(array_bytes(&heap, echoed), b"hello-tls");
        stream_close(&mut heap, s).expect("close");
        handle.join().expect("server thread");
    }

    /// Large payload so rustls may buffer ciphertext across write/flush; ensures
    /// read_to_end still drains pending writes instead of hanging on poll(read).
    #[test]
    fn connect_insecure_large_write_then_read_to_end() {
        let (port, handle) = spawn_tls_echo_server();
        let mut heap = Heap::default();
        let s = tls_connect_insecure(&mut heap, "127.0.0.1", port as i64).expect("connect");
        let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
        let msg = make_byte_array(&mut heap, &payload);
        stream_write_all(&mut heap, s, msg).expect("write_all");
        let echoed = stream_read_to_end(&mut heap, s).expect("read_to_end");
        assert_eq!(array_bytes(&heap, echoed), payload);
        stream_close(&mut heap, s).expect("close");
        handle.join().expect("server thread");
    }

    #[test]
    fn connect_rejects_self_signed_with_verification() {
        let (port, handle) = spawn_tls_echo_server();
        let mut heap = Heap::default();
        let err = tls_connect(&mut heap, "127.0.0.1", port as i64).unwrap_err();
        assert_eq!(err, IoErrorTag::Other);
        handle.join().expect("server thread");
    }

    #[test]
    fn connect_insecure_invalid_port() {
        let mut heap = Heap::default();
        let err = tls_connect_insecure(&mut heap, "127.0.0.1", 99999).unwrap_err();
        assert_eq!(err, IoErrorTag::InvalidInput);
    }

    #[test]
    fn connect_insecure_negative_port() {
        let mut heap = Heap::default();
        let err = tls_connect_insecure(&mut heap, "127.0.0.1", -1).unwrap_err();
        assert_eq!(err, IoErrorTag::InvalidInput);
    }

    #[test]
    fn connect_rejects_empty_server_name() {
        let mut heap = Heap::default();
        let err = tls_connect_insecure(&mut heap, "", 443).unwrap_err();
        assert_eq!(err, IoErrorTag::InvalidInput);
        let err = tls_connect(&mut heap, "", 443).unwrap_err();
        assert_eq!(err, IoErrorTag::InvalidInput);
    }

    #[test]
    fn connect_insecure_connection_refused() {
        // Port 1 is almost never listening on loopback in CI/dev.
        let mut heap = Heap::default();
        let err = tls_connect_insecure(&mut heap, "127.0.0.1", 1).unwrap_err();
        assert_eq!(err, IoErrorTag::Other);
    }

    #[test]
    fn empty_write_then_double_close() {
        let (port, handle) = spawn_tls_echo_server();
        let mut heap = Heap::default();
        let s = tls_connect_insecure(&mut heap, "127.0.0.1", port as i64).expect("connect");
        let empty = make_byte_array(&mut heap, b"");
        stream_write_all(&mut heap, s, empty).expect("empty write_all");
        // Peer waits for app data; close_notify should still succeed.
        stream_close(&mut heap, s).expect("close");
        let err = stream_close(&mut heap, s).unwrap_err();
        assert_eq!(err, IoErrorTag::AlreadyClosed);
        // Server may exit early when the client closes without sending data.
        let _ = handle.join();
    }

    #[test]
    fn verified_connect_invalid_port() {
        let mut heap = Heap::default();
        let err = tls_connect(&mut heap, "localhost", -5).unwrap_err();
        assert_eq!(err, IoErrorTag::InvalidInput);
    }
}
