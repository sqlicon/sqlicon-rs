//! Safe, single-use out-of-band cancellation for prepared DML execution.
//!
//! Ports `libsqli`'s `sqli_cancel_operation` (`include/libsqli/sqli_cancel.h`,
//! `src/sqli_cancel_posix.c`) and the cancelable execute wrapper
//! (`sqli_execute_cancelable` in `src/sqli_prepare.c`). The earlier, general-purpose
//! `CancelHandle` (a bare raw fd that could be fired at any time, against any operation,
//! with no lifetime tie to the `Connection`) is gone: it allowed a cancel to race a dropped
//! connection's fd being reused by the OS for an unrelated socket, and let a caller "cancel"
//! an arbitrary SELECT, which `libsqli` explicitly forbids.
//!
//! The replacement is scoped to exactly one [`crate::Connection::execute_stmt_cancelable`]
//! call on a prepared `UPDATE`/`DELETE`/`INSERT`, restricted to TCP connections, and
//! unconditionally discards the connection after any urgent-send attempt (successful or not) --
//! an `MSG_OOB` send can desynchronize the wire even when it "succeeds", so the connection can
//! never be used again afterwards, matching `libsqli`'s `SQLI_CONN_DISCARDED` semantics.

use crate::error::{Error, Result};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(unix)]
pub(crate) type RawIoHandle = std::os::unix::io::RawFd;
#[cfg(windows)]
pub(crate) type RawIoHandle = std::os::windows::io::RawSocket;

/// `lifecycle` bit: the connection is pinned to an in-flight cancelable operation.
pub(crate) const LC_PINNED: u8 = 1;
/// `lifecycle` bit: the connection is permanently unusable after a cancellation attempt.
pub(crate) const LC_DISCARDED: u8 = 2;

fn send_oob_interrupt(handle: RawIoHandle) -> Result<()> {
    let interrupt_byte = 0x42u8;

    #[cfg(unix)]
    {
        let sent = unsafe {
            libc::send(
                handle,
                &interrupt_byte as *const _ as *const libc::c_void,
                1,
                libc::MSG_OOB | libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if sent == 1 {
            Ok(())
        } else {
            Err(Error::Io(std::io::Error::last_os_error()))
        }
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Networking::WinSock::{send, MSG_OOB, SOCKET, SOCKET_ERROR};
        let sent = unsafe { send(handle as SOCKET, &interrupt_byte as *const u8, 1, MSG_OOB as i32) };
        if sent != SOCKET_ERROR && sent == 1 {
            Ok(())
        } else {
            Err(Error::Io(std::io::Error::last_os_error()))
        }
    }
}

/// Request disposition, independent of execution and transaction outcome. Mirrors
/// `sqli_cancel_disposition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelDisposition {
    /// Recorded before the EXECUTE group finished sending; no urgent byte sent yet.
    Latched,
    /// One urgent byte sent; execution may still succeed.
    Sent,
    /// Urgent send was attempted but failed; the connection is still discarded.
    SendFailed,
    /// A cancellation was already requested; no additional urgent byte was sent.
    AlreadyRequested,
    /// The call had already finished; no connection was touched.
    Completed,
}

/// Terminal execution interpretation; never a transaction-commit/rollback claim. Mirrors
/// `sqli_cancel_outcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CancelOutcome {
    /// Canceled locally before the EXECUTE request was ever sent.
    #[default]
    NotExecuted,
    /// The server reported successful execution, including when cancel raced completion.
    Executed,
    /// The server reported `SQLCODE -213`.
    Interrupted,
    /// Local or server error; see [`CancelSnapshot::operation_error`].
    Failed,
    /// A transport/response failure prevents determining the execution outcome.
    Unknown,
}

/// Owned terminal snapshot of a single cancelable execute call. Mirrors
/// `sqli_cancel_snapshot`.
#[derive(Debug, Clone, Default)]
pub struct CancelSnapshot {
    /// The call passed its precondition checks and registered; not proof SQL was sent.
    pub started: bool,
    /// At least one cancellation request was accepted.
    pub requested: bool,
    /// An urgent send was attempted, successfully or otherwise.
    pub send_attempted: bool,
    /// Meaningful only when `send_attempted` is true.
    pub send_failed: bool,
    /// Description of the local or server error, if any.
    pub operation_error: Option<String>,
    /// Interpretation of the execution result.
    pub outcome: CancelOutcome,
    /// The connection is permanently unusable, even if cleanup failed.
    pub connection_discarded: bool,
    /// Auto-generated `SERIAL`/`SERIAL8`/`BIGSERIAL` key value, if the statement ran to a
    /// terminal `SQ_DONE` (`outcome == Executed`) and inserted into such a column. `None`
    /// for every other outcome, since there is no row to have generated one.
    pub generated_serial: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpState {
    Fresh,
    Active,
    Terminal,
}

struct Registered {
    lifecycle: Arc<AtomicU8>,
    handle: RawIoHandle,
}

struct Inner {
    state: OpState,
    armed: bool,
    requested: bool,
    registered: Option<Registered>,
    snapshot: CancelSnapshot,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            state: OpState::Fresh,
            armed: false,
            requested: false,
            registered: None,
            snapshot: CancelSnapshot::default(),
        }
    }
}

/// Locks `inner`, recovering from poisoning rather than propagating the panic.
///
/// A poisoned lock here would mean some other task panicked while holding it; every
/// critical section under this lock is a handful of panic-free field assignments (no
/// indexing, no further locking, no I/O), so poisoning should not happen in practice. If it
/// ever does, recovering the state is still safe for this type's one safety-critical
/// invariant: the connection-discarded bit is only ever set (`fetch_or`), never cleared, so
/// a recovered (possibly stale) `Inner` cannot cause an already-discarded connection to
/// look safe to reuse.
fn lock_inner(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn send_interrupt_locked(inner: &mut Inner) {
    if let Some(reg) = &inner.registered {
        // Once an urgent-send attempt is made, the connection is discarded unconditionally,
        // matching libsqli: MSG_OOB can desynchronize the wire even on a "successful" send.
        reg.lifecycle.fetch_or(LC_DISCARDED, Ordering::SeqCst);
        inner.snapshot.send_attempted = true;
        inner.snapshot.send_failed = send_oob_interrupt(reg.handle).is_err();
    }
}

/// A single-use cancellation capability for exactly one
/// [`crate::Connection::execute_stmt_cancelable`] call. Create it, hand a [`CancelRequester`]
/// to whichever task or thread should be able to interrupt the call, run the cancelable
/// execute, then inspect the terminal snapshot via [`CancelOperation::snapshot`].
///
/// Never reused across multiple calls: once the bound call finishes, the operation is
/// terminal and a fresh `CancelOperation` is required for the next cancelable execute.
pub struct CancelOperation {
    inner: Arc<Mutex<Inner>>,
}

impl Default for CancelOperation {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelOperation {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Returns a `Send + Sync + Clone` handle that can request cancellation from any task or
    /// thread. This is the only call in this API meant to run somewhere other than the task
    /// driving the bound `Connection`.
    pub fn requester(&self) -> CancelRequester {
        CancelRequester {
            inner: self.inner.clone(),
        }
    }

    /// Returns the terminal snapshot once the bound call has completed.
    /// Returns `Err(Error::InvalidState)` if the call hasn't finished yet.
    pub fn snapshot(&self) -> Result<CancelSnapshot> {
        let inner = lock_inner(&self.inner);
        if inner.state != OpState::Terminal {
            return Err(Error::InvalidState(
                "cancel operation has not completed yet".into(),
            ));
        }
        Ok(inner.snapshot.clone())
    }

    /// Registers this operation against `conn`'s lifecycle, pinning the connection
    /// (CAS 0 -> PINNED, matching `register_operation`/`sqli_cancel_operation_prepare`).
    /// Returns `Ok(true)` if the caller should proceed to send the EXECUTE request,
    /// `Ok(false)` if a cancellation was already latched before registration (the call must
    /// not send anything and is already terminal with `CancelOutcome::NotExecuted`).
    pub(crate) fn register(&self, lifecycle: Arc<AtomicU8>, handle: RawIoHandle) -> Result<bool> {
        let mut inner = lock_inner(&self.inner);
        if inner.state != OpState::Fresh {
            return Err(Error::InvalidState(
                "cancel operation was already used for a previous call".into(),
            ));
        }
        if lifecycle
            .compare_exchange(0, LC_PINNED, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(Error::InvalidState(
                "connection is already pinned to another cancelable call, or has been discarded"
                    .into(),
            ));
        }
        if inner.requested {
            inner.snapshot.started = true;
            inner.snapshot.outcome = CancelOutcome::NotExecuted;
            inner.state = OpState::Terminal;
            lifecycle.fetch_and(!LC_PINNED, Ordering::SeqCst);
            return Ok(false);
        }
        inner.state = OpState::Active;
        inner.snapshot.started = true;
        inner.registered = Some(Registered { lifecycle, handle });
        Ok(true)
    }

    /// Unregisters without discarding the connection: used when sending the bind/execute bytes
    /// itself failed before the EXECUTE group was ever completed, so no ambiguous in-flight DML
    /// exists on the wire and the connection can be safely reused.
    pub(crate) fn abort_unregistered(&self) {
        let mut inner = lock_inner(&self.inner);
        if let Some(reg) = inner.registered.take() {
            reg.lifecycle.fetch_and(!LC_PINNED, Ordering::SeqCst);
        }
        inner.state = OpState::Terminal;
    }

    /// Enables urgent sending for any already-latched or future request. Called right after
    /// the full EXECUTE group has been flushed to the socket, matching
    /// `sqli_cancel_operation_arm`.
    pub(crate) fn arm(&self) {
        let mut inner = lock_inner(&self.inner);
        if inner.state != OpState::Active || inner.armed {
            return;
        }
        inner.armed = true;
        if inner.requested {
            send_interrupt_locked(&mut inner);
        }
    }

    /// Finalizes the operation with the execution outcome, matching
    /// `sqli_cancel_operation_finish`. `discard` additionally forces the connection discarded
    /// (e.g. an ambiguous transport failure after EXECUTE was sent, where it's unknown whether
    /// the DML ran).
    pub(crate) fn finish(
        &self,
        outcome: CancelOutcome,
        discard: bool,
        operation_error: Option<String>,
    ) -> CancelSnapshot {
        let mut inner = lock_inner(&self.inner);
        let Some(reg) = inner.registered.take() else {
            return inner.snapshot.clone();
        };
        inner.snapshot.operation_error = operation_error;
        if discard {
            reg.lifecycle.fetch_or(LC_DISCARDED, Ordering::SeqCst);
        }
        inner.snapshot.connection_discarded = reg.lifecycle.load(Ordering::SeqCst) & LC_DISCARDED != 0;
        inner.snapshot.outcome = outcome;
        inner.state = OpState::Terminal;
        reg.lifecycle.fetch_and(!LC_PINNED, Ordering::SeqCst);
        inner.snapshot.clone()
    }
}

/// `Send + Sync + Clone` handle to request cancellation of the in-flight
/// [`crate::Connection::execute_stmt_cancelable`] call bound to the originating
/// [`CancelOperation`]. This is the only call in this API meant to run on a different task or
/// thread than the one driving the connection; it is not signal-handler safe.
#[derive(Clone)]
pub struct CancelRequester {
    inner: Arc<Mutex<Inner>>,
}

impl CancelRequester {
    /// Requests cancellation. `Ok(_)` means the disposition was recorded, not that execution
    /// actually stopped -- always inspect the operation's terminal snapshot afterwards.
    /// Repeated requests send no further urgent bytes.
    pub fn request(&self) -> CancelDisposition {
        let mut inner = lock_inner(&self.inner);
        if inner.state == OpState::Terminal {
            return CancelDisposition::Completed;
        }
        if inner.requested {
            return CancelDisposition::AlreadyRequested;
        }
        inner.requested = true;
        inner.snapshot.requested = true;
        if inner.state == OpState::Fresh || !inner.armed {
            return CancelDisposition::Latched;
        }
        send_interrupt_locked(&mut inner);
        if inner.snapshot.send_failed {
            CancelDisposition::SendFailed
        } else {
            CancelDisposition::Sent
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;

    /// A connected loopback TCP pair, so register()/arm()/request() can exercise a real
    /// MSG_OOB send without needing an Informix server.
    fn loopback_pair() -> (std::net::TcpStream, std::net::TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn register_rejects_double_registration() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(0));
        let op = CancelOperation::new();
        assert!(op.register(lifecycle.clone(), client.as_raw_fd()).unwrap());
        let err = op.register(lifecycle, client.as_raw_fd()).unwrap_err();
        assert!(matches!(err, Error::InvalidState(_)));
    }

    #[test]
    fn register_rejects_already_pinned_connection() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(LC_PINNED));
        let op = CancelOperation::new();
        let err = op.register(lifecycle, client.as_raw_fd()).unwrap_err();
        assert!(matches!(err, Error::InvalidState(_)));
    }

    #[test]
    fn register_rejects_already_discarded_connection() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(LC_DISCARDED));
        let op = CancelOperation::new();
        let err = op.register(lifecycle, client.as_raw_fd()).unwrap_err();
        assert!(matches!(err, Error::InvalidState(_)));
    }

    #[test]
    fn request_before_arm_only_latches_and_does_not_touch_connection() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(0));
        let op = CancelOperation::new();
        assert!(op.register(lifecycle.clone(), client.as_raw_fd()).unwrap());

        let requester = op.requester();
        assert_eq!(requester.request(), CancelDisposition::Latched);
        // No urgent send happened yet, so the connection must not be discarded.
        assert_eq!(lifecycle.load(Ordering::SeqCst) & LC_DISCARDED, 0);

        // A second request before arming is a duplicate, not a second latch.
        assert_eq!(requester.request(), CancelDisposition::AlreadyRequested);
    }

    #[test]
    fn arm_fires_an_already_latched_request_and_discards() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(0));
        let op = CancelOperation::new();
        assert!(op.register(lifecycle.clone(), client.as_raw_fd()).unwrap());

        let requester = op.requester();
        assert_eq!(requester.request(), CancelDisposition::Latched);

        op.arm();
        // arm() must have sent the urgent byte for the already-latched request.
        assert_ne!(lifecycle.load(Ordering::SeqCst) & LC_DISCARDED, 0);

        let snapshot = op.finish(CancelOutcome::Interrupted, false, None);
        assert!(snapshot.send_attempted);
        assert!(!snapshot.send_failed);
        assert!(snapshot.connection_discarded);
        assert_eq!(lifecycle.load(Ordering::SeqCst) & LC_PINNED, 0);
    }

    #[test]
    fn request_after_arm_sends_immediately() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(0));
        let op = CancelOperation::new();
        assert!(op.register(lifecycle.clone(), client.as_raw_fd()).unwrap());
        op.arm();

        let requester = op.requester();
        assert_eq!(requester.request(), CancelDisposition::Sent);
        assert_ne!(lifecycle.load(Ordering::SeqCst) & LC_DISCARDED, 0);
        assert_eq!(requester.request(), CancelDisposition::AlreadyRequested);
    }

    #[test]
    fn finish_without_any_request_does_not_discard_on_success() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(0));
        let op = CancelOperation::new();
        assert!(op.register(lifecycle.clone(), client.as_raw_fd()).unwrap());
        op.arm();

        let snapshot = op.finish(CancelOutcome::Executed, false, None);
        assert!(!snapshot.send_attempted);
        assert!(!snapshot.connection_discarded);
        assert_eq!(lifecycle.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn finish_can_force_discard_on_ambiguous_outcome() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(0));
        let op = CancelOperation::new();
        assert!(op.register(lifecycle.clone(), client.as_raw_fd()).unwrap());
        op.arm();

        let snapshot = op.finish(CancelOutcome::Unknown, true, Some("io error".into()));
        assert!(snapshot.connection_discarded);
        assert_eq!(lifecycle.load(Ordering::SeqCst) & LC_DISCARDED, LC_DISCARDED);
        assert_eq!(snapshot.operation_error, Some("io error".into()));
    }

    #[test]
    fn requesting_before_registration_short_circuits_the_bound_call() {
        let (client, _server) = loopback_pair();
        let lifecycle = Arc::new(AtomicU8::new(0));
        let op = CancelOperation::new();

        let requester = op.requester();
        assert_eq!(requester.request(), CancelDisposition::Latched);

        // register() must now report "don't send anything" and go straight to terminal.
        assert!(!op.register(lifecycle.clone(), client.as_raw_fd()).unwrap());
        let snapshot = op.snapshot().unwrap();
        assert_eq!(snapshot.outcome, CancelOutcome::NotExecuted);
        // The connection was briefly pinned and then unpinned, never discarded.
        assert_eq!(lifecycle.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn snapshot_before_terminal_errors() {
        let op = CancelOperation::new();
        assert!(matches!(op.snapshot().unwrap_err(), Error::InvalidState(_)));
    }
}
