//! IO readiness reactor — sibling of the CPU work-stealing [`crate::reactor::Reactor`].
//!
//! Sync adapters (`read_exact`, `write_all`, …) and TLS handshake waits block here
//! via single-fd `poll` (works for sockets, pipes, and regular files). Async waiters
//! register interest and are woken when [`IoReactor::poll_once`] observes readiness
//! (Phase 2 cooperative / help-steal paths).

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::io::IoErrorTag;

/// Readiness interest for a file descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interest {
    Readable,
    Writable,
}

impl Interest {
    fn poll_events(self) -> libc::c_short {
        match self {
            Self::Readable => libc::POLLIN,
            Self::Writable => libc::POLLOUT,
        }
    }
}

/// Token identifying an async waiter registered with the reactor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaitToken(u64);

/// One async readiness subscription.
struct AsyncWait {
    fd: RawFd,
    interest: Interest,
    done: bool,
}

struct Inner {
    next_token: AtomicU64,
    waits: Mutex<HashMap<WaitToken, AsyncWait>>,
    ready: Mutex<Vec<WaitToken>>,
    cvar: Condvar,
}

/// Per-root-VM IO readiness reactor.
pub struct IoReactor {
    inner: Inner,
}

impl IoReactor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Inner {
                next_token: AtomicU64::new(1),
                waits: Mutex::new(HashMap::new()),
                ready: Mutex::new(Vec::new()),
                cvar: Condvar::new(),
            },
        })
    }

    /// Block until `fd` is ready for `interest`, or `timeout` elapses.
    ///
    /// Used by sync adapters and TLS handshake. Prefer
    /// [`Self::wait_fd_helping`] when a CPU reactor is available so fork-join
    /// work can progress during the wait.
    pub fn wait_fd(
        &self,
        fd: RawFd,
        interest: Interest,
        timeout: Option<Duration>,
    ) -> Result<(), IoErrorTag> {
        poll_one(fd, interest, timeout)
    }

    /// Like [`Self::wait_fd`], but invokes `help` between short poll slices so
    /// the caller can steal CPU jobs / drive other work (true async overlap).
    pub fn wait_fd_helping(
        &self,
        fd: RawFd,
        interest: Interest,
        timeout: Option<Duration>,
        mut help: impl FnMut(),
    ) -> Result<(), IoErrorTag> {
        let deadline = timeout.map(|d| Instant::now() + d);
        // Short slices keep help responsive without spinning.
        const SLICE: Duration = Duration::from_millis(1);
        loop {
            let slice = match deadline {
                None => Some(SLICE),
                Some(end) => {
                    let now = Instant::now();
                    if now >= end {
                        return Err(IoErrorTag::TimedOut);
                    }
                    Some((end - now).min(SLICE))
                }
            };
            match poll_one(fd, interest, slice) {
                Ok(()) => return Ok(()),
                Err(IoErrorTag::TimedOut) => {
                    help();
                    if deadline.is_none() {
                        // Infinite wait: TimedOut on slice means not ready yet.
                        continue;
                    }
                    let now = Instant::now();
                    if let Some(end) = deadline {
                        if now >= end {
                            return Err(IoErrorTag::TimedOut);
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Register an async waiter; returns a token woken by [`Self::poll_once`].
    pub fn register_wait(&self, fd: RawFd, interest: Interest) -> WaitToken {
        let token = WaitToken(self.inner.next_token.fetch_add(1, Ordering::Relaxed));
        self.inner
            .waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                token,
                AsyncWait {
                    fd,
                    interest,
                    done: false,
                },
            );
        token
    }

    /// Cancel a waiter (e.g. stream closed); safe if already ready.
    pub fn cancel_wait(&self, token: WaitToken) {
        self.inner
            .waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&token);
    }

    /// Block until `token` is marked ready (or cancelled → TimedOut-like Other).
    pub fn wait_token(&self, token: WaitToken, timeout: Option<Duration>) -> Result<(), IoErrorTag> {
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut ready = self.inner.ready.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if ready.iter().any(|t| *t == token) {
                ready.retain(|t| *t != token);
                self.inner
                    .waits
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&token);
                return Ok(());
            }
            // Still registered?
            if !self
                .inner
                .waits
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&token)
            {
                return Err(IoErrorTag::Other);
            }
            let wait_dur = match deadline {
                None => Duration::from_millis(50),
                Some(end) => {
                    let now = Instant::now();
                    if now >= end {
                        self.cancel_wait(token);
                        return Err(IoErrorTag::TimedOut);
                    }
                    (end - now).min(Duration::from_millis(50))
                }
            };
            // Drive readiness while waiting.
            drop(ready);
            let _ = self.poll_once(Some(Duration::ZERO));
            ready = self.inner.ready.lock().unwrap_or_else(|e| e.into_inner());
            let (guard, timed_out) = self
                .inner
                .cvar
                .wait_timeout(ready, wait_dur)
                .unwrap_or_else(|e| e.into_inner());
            ready = guard;
            if timed_out.timed_out() {
                let _ = self.poll_once(Some(Duration::ZERO));
            }
        }
    }

    /// Poll registered waiters once; marks ready tokens and notifies.
    ///
    /// Returns the number of newly ready waiters.
    pub fn poll_once(&self, timeout: Option<Duration>) -> usize {
        let snapshot: Vec<(WaitToken, RawFd, Interest)> = {
            let waits = self.inner.waits.lock().unwrap_or_else(|e| e.into_inner());
            waits
                .iter()
                .filter(|(_, w)| !w.done)
                .map(|(t, w)| (*t, w.fd, w.interest))
                .collect()
        };
        if snapshot.is_empty() {
            if let Some(d) = timeout {
                if !d.is_zero() {
                    std::thread::sleep(d.min(Duration::from_millis(1)));
                }
            }
            return 0;
        }
        let mut pfds: Vec<libc::pollfd> = snapshot
            .iter()
            .map(|(_, fd, interest)| libc::pollfd {
                fd: *fd,
                events: interest.poll_events(),
                revents: 0,
            })
            .collect();
        let timeout_ms = match timeout {
            None => 0, // non-blocking by default for poll_once
            Some(d) if d.is_zero() => 0,
            Some(d) => d.as_millis().min(i32::MAX as u128) as i32,
        };
        let rc = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
        if rc <= 0 {
            return 0;
        }
        let mut n = 0usize;
        let mut waits = self.inner.waits.lock().unwrap_or_else(|e| e.into_inner());
        let mut ready = self.inner.ready.lock().unwrap_or_else(|e| e.into_inner());
        for (i, (token, _, _)) in snapshot.iter().enumerate() {
            let rev = pfds[i].revents;
            if rev == 0 {
                continue;
            }
            if let Some(w) = waits.get_mut(token) {
                if !w.done {
                    w.done = true;
                    ready.push(*token);
                    n += 1;
                }
            }
        }
        if n > 0 {
            self.inner.cvar.notify_all();
        }
        n
    }
}

impl Default for IoReactor {
    fn default() -> Self {
        // Prefer Arc::new via IoReactor::new for sharing; Default for tests.
        Self {
            inner: Inner {
                next_token: AtomicU64::new(1),
                waits: Mutex::new(HashMap::new()),
                ready: Mutex::new(Vec::new()),
                cvar: Condvar::new(),
            },
        }
    }
}

fn poll_one(
    fd: RawFd,
    interest: Interest,
    timeout: Option<Duration>,
) -> Result<(), IoErrorTag> {
    let mut pfd = libc::pollfd {
        fd,
        events: interest.poll_events(),
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
            return Err(IoErrorTag::from_kind(err.kind()));
        }
        if rc == 0 {
            return Err(IoErrorTag::TimedOut);
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, IntoRawFd};
    use std::net::TcpListener;

    #[test]
    fn wait_fd_accepts_listening_socket_as_writable_or_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let fd = listener.as_raw_fd();
        let io = IoReactor::new();
        let _ = io.wait_fd(fd, Interest::Writable, Some(Duration::from_millis(50)));
        std::mem::forget(listener);
    }

    #[test]
    fn async_register_and_poll() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let fd = listener.as_raw_fd();
        let io = IoReactor::new();
        let tok = io.register_wait(fd, Interest::Readable);
        assert_eq!(io.poll_once(Some(Duration::from_millis(10))), 0);
        io.cancel_wait(tok);
        std::mem::forget(listener);
    }
}
