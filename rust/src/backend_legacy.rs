//! Explicit Unix-socket compatibility client for the complete v0.4 pair.
//!
//! This module has no default socket path and performs no discovery. Merely
//! constructing a client does not touch the filesystem or a running service.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::mem;
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use crate::backend::{
    AuraControlTransport, PairActionFailure, PairActionReceipt, PairActionResult, PairBackend,
    PairBackendError, PairBackendEvidence, PairBackendKind, PairHealth, PairLifecycle,
};

const MAX_LEGACY_RESPONSE_BYTES: usize = 16 * 1024;
const BACKLOG_RETRY_DELAY: Duration = Duration::from_millis(2);

#[derive(Debug, Clone, Copy)]
enum LegacyCommand {
    Status,
    Start,
    Stop,
    Shutdown,
}

impl LegacyCommand {
    const fn wire_line(self) -> &'static [u8] {
        match self {
            Self::Status => b"status\n",
            Self::Start => b"start\n",
            Self::Stop => b"stop\n",
            Self::Shutdown => b"shutdown\n",
        }
    }

    const fn expected_lifecycle(self) -> Option<PairLifecycle> {
        match self {
            Self::Status => None,
            Self::Start => Some(PairLifecycle::Linked),
            Self::Stop => Some(PairLifecycle::Ready),
            Self::Shutdown => Some(PairLifecycle::ShuttingDown),
        }
    }
}

#[derive(Debug, Deserialize)]
struct LegacyWireResponse {
    ok: bool,
    #[serde(default)]
    state: Option<Value>,
    #[serde(default)]
    last_error: Option<Value>,
    #[serde(default)]
    aura_transport: Option<Value>,
    #[serde(default)]
    idempotent: Option<Value>,
}

struct ParsedLegacyResponse {
    lifecycle: PairLifecycle,
    reported_error: bool,
    aura_transport: AuraControlTransport,
}

/// Compatibility backend for the complete v0.4 JBL + Aura supervisor.
///
/// The constructor requires an explicit absolute socket path. There is no
/// `Default`, environment lookup, runtime-directory lookup, daemon launcher or
/// service probe in this module.
pub struct LegacyV04PairBackend {
    socket_path: PathBuf,
    timeout: Duration,
}

impl fmt::Debug for LegacyV04PairBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyV04PairBackend")
            .field("socket_path", &"<redacted>")
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl LegacyV04PairBackend {
    pub fn for_explicit_socket(
        socket_path: impl Into<PathBuf>,
        timeout: Duration,
    ) -> Result<Self, PairBackendError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(PairBackendError::InvalidSocketPath);
        }
        if timeout.is_zero() {
            return Err(PairBackendError::InvalidTimeout);
        }
        Ok(Self {
            socket_path,
            timeout,
        })
    }

    fn request(&self, command: LegacyCommand) -> Result<ParsedLegacyResponse, PairBackendError> {
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or(PairBackendError::InvalidTimeout)?;
        let mut stream = connect_trusted_until(&self.socket_path, deadline)?;
        write_all_until(&mut stream, command.wire_line(), deadline)?;
        stream.shutdown(Shutdown::Write).map_err(map_io_error)?;

        let payload = read_one_bounded_reply(&mut stream, deadline)?;
        let response: LegacyWireResponse =
            serde_json::from_slice(&payload).map_err(|_| PairBackendError::InvalidResponse)?;
        if !response.ok {
            return Err(PairBackendError::BackendReportedFailure);
        }

        let lifecycle = parse_lifecycle(response.state.as_ref())?;
        if let Some(expected) = command.expected_lifecycle() {
            if lifecycle != expected {
                return Err(PairBackendError::UnexpectedLifecycle);
            }
        }

        Ok(ParsedLegacyResponse {
            lifecycle,
            reported_error: projected_error_presence(response.last_error.as_ref()),
            aura_transport: projected_transport(response.aura_transport.as_ref()),
        })
    }

    fn action(&self, command: LegacyCommand) -> PairActionResult {
        let deadline = match Instant::now().checked_add(self.timeout) {
            Some(deadline) => deadline,
            None => return self.rejected_before_send(PairBackendError::InvalidTimeout),
        };
        let mut stream = match connect_trusted_until(&self.socket_path, deadline) {
            Ok(stream) => stream,
            Err(reason) => return self.rejected_before_send(reason),
        };

        // From this exact point onward even an error from the first write call
        // cannot prove that the peer observed zero bytes. Keep every failure in
        // the non-retryable OutcomeUnknown branch.
        if let Err(reason) = write_all_until(&mut stream, command.wire_line(), deadline) {
            return self.outcome_unknown(reason, None);
        }
        if let Err(error) = stream.shutdown(Shutdown::Write) {
            return self.outcome_unknown(map_io_error(error), None);
        }
        let payload = match read_one_bounded_reply(&mut stream, deadline) {
            Ok(payload) => payload,
            Err(reason) => return self.outcome_unknown(reason, None),
        };
        let response: LegacyWireResponse = match serde_json::from_slice(&payload) {
            Ok(response) => response,
            Err(_) => return self.outcome_unknown(PairBackendError::InvalidResponse, None),
        };

        let observed_lifecycle = parse_lifecycle(response.state.as_ref()).ok();
        if !response.ok {
            // v0.4 may report ok:false after running part of a multi-device
            // sequence. It is not proof that no device-side write occurred.
            return self
                .outcome_unknown(PairBackendError::BackendReportedFailure, observed_lifecycle);
        }
        let lifecycle = match observed_lifecycle {
            Some(lifecycle) => lifecycle,
            None => return self.outcome_unknown(PairBackendError::InvalidLifecycle, None),
        };
        if command.expected_lifecycle() != Some(lifecycle) {
            return self.outcome_unknown(PairBackendError::UnexpectedLifecycle, Some(lifecycle));
        }
        let idempotent = match parse_optional_bool(response.idempotent.as_ref()) {
            Ok(idempotent) => idempotent,
            Err(reason) => return self.outcome_unknown(reason, Some(lifecycle)),
        };

        let evidence = if idempotent {
            PairBackendEvidence::LocalSessionState
        } else {
            PairBackendEvidence::LifecycleAcknowledgement
        };
        PairActionResult::Accepted(PairActionReceipt::new(
            PairBackendKind::LegacyV04WholePair,
            lifecycle,
            evidence,
            idempotent,
        ))
    }

    const fn rejected_before_send(&self, reason: PairBackendError) -> PairActionResult {
        PairActionResult::RejectedBeforeSend(PairActionFailure::new(
            PairBackendKind::LegacyV04WholePair,
            reason,
            None,
        ))
    }

    const fn outcome_unknown(
        &self,
        reason: PairBackendError,
        observed_lifecycle: Option<PairLifecycle>,
    ) -> PairActionResult {
        PairActionResult::OutcomeUnknown(PairActionFailure::new(
            PairBackendKind::LegacyV04WholePair,
            reason,
            observed_lifecycle,
        ))
    }
}

impl PairBackend for LegacyV04PairBackend {
    fn kind(&self) -> PairBackendKind {
        PairBackendKind::LegacyV04WholePair
    }

    fn health(&mut self) -> Result<PairHealth, PairBackendError> {
        let response = self.request(LegacyCommand::Status)?;
        Ok(PairHealth::new(
            self.kind(),
            response.lifecycle,
            response.reported_error,
            response.aura_transport,
        ))
    }

    fn start(&mut self) -> PairActionResult {
        self.action(LegacyCommand::Start)
    }

    fn stop(&mut self) -> PairActionResult {
        self.action(LegacyCommand::Stop)
    }

    fn shutdown(&mut self) -> PairActionResult {
        self.action(LegacyCommand::Shutdown)
    }
}

fn connect_trusted_until(
    socket_path: &Path,
    deadline: Instant,
) -> Result<UnixStream, PairBackendError> {
    let (address, address_length) = socket_address(socket_path)?;
    inspect_socket_path(socket_path)?;

    let raw_descriptor = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if raw_descriptor < 0 {
        return Err(map_io_error(io::Error::last_os_error()));
    }
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw_descriptor) };
    connect_descriptor(descriptor.as_raw_fd(), &address, address_length, deadline)?;
    set_blocking(descriptor.as_raw_fd())?;
    let stream = unsafe { UnixStream::from_raw_fd(descriptor.into_raw_fd()) };
    verify_peer_effective_uid(&stream)?;
    Ok(stream)
}

fn inspect_socket_path(socket_path: &Path) -> Result<(), PairBackendError> {
    let metadata = fs::symlink_metadata(socket_path).map_err(map_io_error)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(PairBackendError::UntrustedSocket);
    }
    Ok(())
}

fn socket_address(
    socket_path: &Path,
) -> Result<(libc::sockaddr_un, libc::socklen_t), PairBackendError> {
    let path = socket_path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    if path.is_empty() || path.contains(&0) || path.len() >= address.sun_path.len() {
        return Err(PairBackendError::InvalidSocketPath);
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(path) {
        *target = *source as libc::c_char;
    }
    let length = mem::offset_of!(libc::sockaddr_un, sun_path) + path.len() + 1;
    let length =
        libc::socklen_t::try_from(length).map_err(|_| PairBackendError::InvalidSocketPath)?;
    Ok((address, length))
}

fn connect_descriptor(
    descriptor: RawFd,
    address: &libc::sockaddr_un,
    address_length: libc::socklen_t,
    deadline: Instant,
) -> Result<(), PairBackendError> {
    loop {
        let result = unsafe {
            libc::connect(
                descriptor,
                std::ptr::from_ref(address).cast::<libc::sockaddr>(),
                address_length,
            )
        };
        if result == 0 {
            return Ok(());
        }

        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EISCONN) => return Ok(()),
            Some(libc::EINPROGRESS) | Some(libc::EALREADY) => {
                return wait_for_connect(descriptor, deadline);
            }
            // Linux reports EAGAIN for a nonblocking AF_UNIX connect when the
            // listener backlog is full. Unlike EINPROGRESS, that attempt did
            // not enter a pollable in-progress state, so retry it until the
            // same absolute deadline rather than treating SO_ERROR=0 as done.
            Some(libc::EAGAIN) => {
                let remaining = remaining_before(deadline)?;
                thread::sleep(remaining.min(BACKLOG_RETRY_DELAY));
            }
            _ => return Err(map_io_error(error)),
        }
    }
}

fn wait_for_connect(descriptor: RawFd, deadline: Instant) -> Result<(), PairBackendError> {
    loop {
        let remaining = remaining_before(deadline)?;
        let timeout_millis = poll_timeout_millis(remaining);
        let mut poll_descriptor = libc::pollfd {
            fd: descriptor,
            events: libc::POLLOUT,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut poll_descriptor, 1, timeout_millis) };
        if result == 0 {
            return Err(PairBackendError::TimedOut);
        }
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(map_io_error(error));
        }
        if poll_descriptor.revents & libc::POLLNVAL != 0 {
            return Err(PairBackendError::Unavailable);
        }

        let error = socket_error(descriptor)?;
        match error {
            0 => return Ok(()),
            libc::EINPROGRESS | libc::EALREADY => continue,
            libc::EAGAIN => return Err(PairBackendError::TimedOut),
            _ => {
                return Err(map_io_error(io::Error::from_raw_os_error(error)));
            }
        }
    }
}

fn remaining_before(deadline: Instant) -> Result<Duration, PairBackendError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(PairBackendError::TimedOut)
    } else {
        Ok(remaining)
    }
}

fn poll_timeout_millis(timeout: Duration) -> libc::c_int {
    let rounded_up = timeout.as_millis().saturating_add(1);
    rounded_up.min(libc::c_int::MAX as u128) as libc::c_int
}

fn socket_error(descriptor: RawFd) -> Result<libc::c_int, PairBackendError> {
    let mut error: libc::c_int = 0;
    let mut length = libc::socklen_t::try_from(mem::size_of_val(&error))
        .map_err(|_| PairBackendError::Unavailable)?;
    let result = unsafe {
        libc::getsockopt(
            descriptor,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            std::ptr::from_mut(&mut error).cast(),
            &mut length,
        )
    };
    if result == 0 {
        Ok(error)
    } else {
        Err(map_io_error(io::Error::last_os_error()))
    }
}

fn set_blocking(descriptor: RawFd) -> Result<(), PairBackendError> {
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(map_io_error(io::Error::last_os_error()));
    }
    if unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(map_io_error(io::Error::last_os_error()));
    }
    Ok(())
}

fn verify_peer_effective_uid(stream: &UnixStream) -> Result<(), PairBackendError> {
    let mut credentials: libc::ucred = unsafe { mem::zeroed() };
    let expected_length = libc::socklen_t::try_from(mem::size_of_val(&credentials))
        .map_err(|_| PairBackendError::UntrustedSocket)?;
    let mut length = expected_length;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut credentials).cast(),
            &mut length,
        )
    };
    if result != 0
        || length != expected_length
        || !peer_uid_is_trusted(credentials.uid, unsafe { libc::geteuid() })
    {
        return Err(PairBackendError::UntrustedSocket);
    }
    Ok(())
}

const fn peer_uid_is_trusted(peer_uid: libc::uid_t, effective_uid: libc::uid_t) -> bool {
    peer_uid == effective_uid
}

fn map_io_error(error: io::Error) -> PairBackendError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => PairBackendError::TimedOut,
        _ => PairBackendError::Unavailable,
    }
}

fn write_all_until(
    stream: &mut UnixStream,
    mut payload: &[u8],
    deadline: Instant,
) -> Result<(), PairBackendError> {
    while !payload.is_empty() {
        stream
            .set_write_timeout(Some(remaining_before(deadline)?))
            .map_err(map_io_error)?;
        match stream.write(payload) {
            Ok(0) => return Err(PairBackendError::Unavailable),
            Ok(count) => payload = &payload[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(map_io_error(error)),
        }
    }
    Ok(())
}

fn read_one_bounded_reply(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<Vec<u8>, PairBackendError> {
    let mut payload = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        stream
            .set_read_timeout(Some(remaining_before(deadline)?))
            .map_err(map_io_error)?;
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(map_io_error(error)),
        };
        if count == 0 {
            return Err(PairBackendError::InvalidResponse);
        }
        if let Some(newline) = chunk[..count].iter().position(|byte| *byte == b'\n') {
            if payload.len() + newline > MAX_LEGACY_RESPONSE_BYTES {
                return Err(PairBackendError::ResponseTooLarge);
            }
            payload.extend_from_slice(&chunk[..newline]);
            if chunk[newline + 1..count]
                .iter()
                .any(|byte| !byte.is_ascii_whitespace())
            {
                return Err(PairBackendError::InvalidResponse);
            }
            return Ok(payload);
        }
        if payload.len() + count > MAX_LEGACY_RESPONSE_BYTES {
            return Err(PairBackendError::ResponseTooLarge);
        }
        payload.extend_from_slice(&chunk[..count]);
    }
}

fn parse_lifecycle(value: Option<&Value>) -> Result<PairLifecycle, PairBackendError> {
    let state = value
        .and_then(Value::as_str)
        .ok_or(PairBackendError::InvalidLifecycle)?;
    match state {
        "offline" => Ok(PairLifecycle::Offline),
        "initializing" => Ok(PairLifecycle::Initializing),
        "connecting" => Ok(PairLifecycle::Connecting),
        "ready" => Ok(PairLifecycle::Ready),
        "starting" => Ok(PairLifecycle::Linking),
        "linked" => Ok(PairLifecycle::Linked),
        "stopping" => Ok(PairLifecycle::Unlinking),
        "degraded" => Ok(PairLifecycle::Degraded),
        "recovering" => Ok(PairLifecycle::Recovering),
        "shutting-down" => Ok(PairLifecycle::ShuttingDown),
        "failed" => Ok(PairLifecycle::Failed),
        _ => Err(PairBackendError::InvalidLifecycle),
    }
}

fn projected_error_presence(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::String(message)) => !message.is_empty(),
        Some(_) => true,
    }
}

fn projected_transport(value: Option<&Value>) -> AuraControlTransport {
    match value.and_then(Value::as_str) {
        Some("le") => AuraControlTransport::Le,
        Some("bredr") => AuraControlTransport::BrEdr,
        Some("unresolved") => AuraControlTransport::Unresolved,
        _ => AuraControlTransport::Unknown,
    }
}

fn parse_optional_bool(value: Option<&Value>) -> Result<bool, PairBackendError> {
    match value {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(PairBackendError::InvalidResponse),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread::{self, JoinHandle};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestSocket {
        directory: PathBuf,
        path: PathBuf,
    }

    impl TestSocket {
        fn unique() -> Self {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "jbl-aura-link-legacy-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&directory).expect("create isolated test directory");
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("restrict isolated test directory");
            let path = directory.join("mock.sock");
            Self { directory, path }
        }
    }

    impl Drop for TestSocket {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_dir(&self.directory);
        }
    }

    struct MockReply {
        payload: Vec<u8>,
        delay: Duration,
    }

    impl MockReply {
        fn immediate(payload: impl Into<Vec<u8>>) -> Self {
            Self {
                payload: payload.into(),
                delay: Duration::ZERO,
            }
        }
    }

    fn spawn_mock(socket: &TestSocket, replies: Vec<MockReply>) -> JoinHandle<Vec<Vec<u8>>> {
        let listener = UnixListener::bind(&socket.path).expect("bind mock Unix socket");
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))
            .expect("restrict mock Unix socket");
        thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut connection, _) = listener.accept().expect("accept mock client");
                connection
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("set mock timeout");
                let mut request = Vec::new();
                connection
                    .read_to_end(&mut request)
                    .expect("read exactly one request");
                requests.push(request);
                if !reply.delay.is_zero() {
                    thread::sleep(reply.delay);
                }
                let _ = connection.write_all(&reply.payload);
            }
            requests
        })
    }

    fn client(socket: &TestSocket) -> LegacyV04PairBackend {
        LegacyV04PairBackend::for_explicit_socket(&socket.path, Duration::from_secs(1))
            .expect("explicit mock path should be accepted")
    }

    fn accepted(result: PairActionResult) -> PairActionReceipt {
        match result {
            PairActionResult::Accepted(receipt) => receipt,
            PairActionResult::RejectedBeforeSend(_) | PairActionResult::OutcomeUnknown(_) => {
                panic!("expected an accepted lifecycle action")
            }
        }
    }

    #[test]
    fn constructor_requires_an_explicit_absolute_path_and_is_lazy() {
        assert_eq!(
            LegacyV04PairBackend::for_explicit_socket("relative.sock", Duration::from_secs(1))
                .unwrap_err(),
            PairBackendError::InvalidSocketPath
        );

        let socket = TestSocket::unique();
        let mut backend = client(&socket);
        assert!(!format!("{backend:?}").contains(&socket.path.to_string_lossy().to_string()));
        assert_eq!(backend.health(), Err(PairBackendError::Unavailable));
    }

    #[test]
    fn stale_socket_without_a_listener_fails_without_waiting_for_io_timeout() {
        let socket = TestSocket::unique();
        let listener = UnixListener::bind(&socket.path).expect("bind stale socket fixture");
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))
            .expect("restrict stale socket fixture");
        drop(listener);

        let mut backend =
            LegacyV04PairBackend::for_explicit_socket(&socket.path, Duration::from_secs(1))
                .expect("explicit stale path should be accepted");
        let started = Instant::now();
        assert_eq!(backend.health(), Err(PairBackendError::Unavailable));
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn saturated_listener_backlog_hits_the_connect_deadline() {
        let socket = TestSocket::unique();
        let listener = UnixListener::bind(&socket.path).expect("bind saturated socket fixture");
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))
            .expect("restrict saturated socket fixture");
        let listen_result = unsafe { libc::listen(listener.as_raw_fd(), 0) };
        assert_eq!(listen_result, 0, "shrink Unix listener backlog");
        let pending = UnixStream::connect(&socket.path).expect("fill the only backlog slot");

        let mut backend =
            LegacyV04PairBackend::for_explicit_socket(&socket.path, Duration::from_millis(35))
                .expect("explicit saturated path should be accepted");
        let started = Instant::now();
        assert_eq!(backend.health(), Err(PairBackendError::TimedOut));
        assert!(started.elapsed() < Duration::from_millis(500));
        drop(pending);
        drop(listener);
    }

    #[test]
    fn symlink_non_socket_and_broad_socket_modes_are_rejected() {
        let socket = TestSocket::unique();
        let listener = UnixListener::bind(&socket.path).expect("bind trusted socket fixture");
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))
            .expect("restrict trusted socket fixture");
        let alias = socket.directory.join("alias.sock");
        symlink(&socket.path, &alias).expect("create socket symlink fixture");
        let mut backend =
            LegacyV04PairBackend::for_explicit_socket(&alias, Duration::from_millis(50))
                .expect("explicit symlink path passes constructor only");
        assert_eq!(backend.health(), Err(PairBackendError::UntrustedSocket));
        fs::remove_file(&alias).expect("remove socket symlink fixture");

        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o660))
            .expect("broaden socket mode fixture");
        let mut backend = client(&socket);
        assert_eq!(backend.health(), Err(PairBackendError::UntrustedSocket));
        drop(listener);

        fs::remove_file(&socket.path).expect("remove socket fixture");
        fs::write(&socket.path, b"not a socket").expect("create regular file fixture");
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))
            .expect("restrict regular file fixture");
        let mut backend = client(&socket);
        assert_eq!(backend.health(), Err(PairBackendError::UntrustedSocket));
    }

    #[test]
    fn peer_credentials_require_the_current_effective_uid() {
        let effective_uid = unsafe { libc::geteuid() };
        assert!(peer_uid_is_trusted(effective_uid, effective_uid));
        assert!(!peer_uid_is_trusted(
            effective_uid.wrapping_add(1),
            effective_uid
        ));
    }

    #[test]
    fn one_connection_carries_one_request_and_one_reply_for_every_command() {
        let socket = TestSocket::unique();
        let server = spawn_mock(
            &socket,
            vec![
                MockReply::immediate(
                    b"{\"ok\":true,\"state\":\"ready\",\"last_error\":null,\"aura_transport\":\"le\"}\n",
                ),
                MockReply::immediate(b"{\"ok\":true,\"state\":\"linked\"}\n"),
                MockReply::immediate(b"{\"ok\":true,\"state\":\"ready\"}\n"),
                MockReply::immediate(b"{\"ok\":true,\"state\":\"shutting-down\"}\n"),
            ],
        );
        let mut backend = client(&socket);
        let mut transaction = crate::backend::PairBackendTransaction::new(&mut backend);

        let health = transaction.health().expect("status should parse");
        assert_eq!(health.lifecycle(), PairLifecycle::Ready);
        assert_eq!(health.level(), crate::backend::PairHealthLevel::Healthy);
        assert_eq!(health.aura_transport(), AuraControlTransport::Le);
        assert_eq!(
            accepted(transaction.start()).lifecycle(),
            PairLifecycle::Linked
        );
        assert_eq!(
            accepted(transaction.stop()).lifecycle(),
            PairLifecycle::Ready
        );
        assert_eq!(
            accepted(transaction.shutdown()).lifecycle(),
            PairLifecycle::ShuttingDown
        );

        let requests = server.join().expect("mock server should finish");
        assert_eq!(
            requests,
            vec![
                b"status\n".to_vec(),
                b"start\n".to_vec(),
                b"stop\n".to_vec(),
                b"shutdown\n".to_vec(),
            ]
        );
    }

    #[test]
    fn idempotent_reply_is_only_local_session_state() {
        let socket = TestSocket::unique();
        let server = spawn_mock(
            &socket,
            vec![MockReply::immediate(
                b"{\"ok\":true,\"state\":\"linked\",\"idempotent\":true}\n",
            )],
        );
        let mut backend = client(&socket);
        let receipt = accepted(backend.start());
        assert!(receipt.is_idempotent());
        assert_eq!(receipt.evidence(), PairBackendEvidence::LocalSessionState);
        server.join().expect("mock server should finish");
    }

    #[test]
    fn action_connect_failure_is_rejected_before_any_send_attempt() {
        let socket = TestSocket::unique();
        let mut backend = client(&socket);

        assert_eq!(
            backend.start(),
            PairActionResult::RejectedBeforeSend(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                PairBackendError::Unavailable,
                None,
            ))
        );
    }

    #[test]
    fn action_timeout_after_request_delivery_is_outcome_unknown() {
        let socket = TestSocket::unique();
        let server = spawn_mock(
            &socket,
            vec![MockReply {
                payload: b"{\"ok\":true,\"state\":\"linked\"}\n".to_vec(),
                delay: Duration::from_millis(200),
            }],
        );
        let mut backend =
            LegacyV04PairBackend::for_explicit_socket(&socket.path, Duration::from_millis(30))
                .expect("explicit mock path should be accepted");

        assert_eq!(
            backend.start(),
            PairActionResult::OutcomeUnknown(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                PairBackendError::TimedOut,
                None,
            ))
        );
        assert_eq!(
            server.join().expect("mock server should finish"),
            vec![b"start\n".to_vec()]
        );
    }

    #[test]
    fn action_disconnect_malformed_and_oversized_replies_are_outcome_unknown() {
        let cases = [
            (Vec::new(), PairBackendError::InvalidResponse),
            (b"not-json\n".to_vec(), PairBackendError::InvalidResponse),
            (
                {
                    let mut payload = vec![b'x'; MAX_LEGACY_RESPONSE_BYTES + 1];
                    payload.push(b'\n');
                    payload
                },
                PairBackendError::ResponseTooLarge,
            ),
        ];

        for (payload, reason) in cases {
            let socket = TestSocket::unique();
            let server = spawn_mock(&socket, vec![MockReply::immediate(payload)]);
            let mut backend = client(&socket);
            assert_eq!(
                backend.start(),
                PairActionResult::OutcomeUnknown(PairActionFailure::new(
                    PairBackendKind::LegacyV04WholePair,
                    reason,
                    None,
                ))
            );
            assert_eq!(
                server.join().expect("mock server should finish"),
                vec![b"start\n".to_vec()]
            );
        }
    }

    #[test]
    fn negative_ack_is_unknown_and_preserves_sanitized_observed_lifecycle() {
        let marker = "sensitive-marker-must-not-escape";
        let socket = TestSocket::unique();
        let payload = format!("{{\"ok\":false,\"state\":\"failed\",\"error\":\"{marker}\"}}\n");
        let server = spawn_mock(&socket, vec![MockReply::immediate(payload.into_bytes())]);
        let mut backend = client(&socket);

        let result = backend.start();
        assert_eq!(
            result,
            PairActionResult::OutcomeUnknown(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                PairBackendError::BackendReportedFailure,
                Some(PairLifecycle::Failed),
            ))
        );
        assert!(!format!("{result:?}").contains(marker));
        server.join().expect("mock server should finish");
    }

    #[test]
    fn timeout_is_typed_and_does_not_echo_socket_details() {
        let socket = TestSocket::unique();
        let server = spawn_mock(
            &socket,
            vec![MockReply {
                payload: b"{\"ok\":true,\"state\":\"ready\"}\n".to_vec(),
                delay: Duration::from_millis(200),
            }],
        );
        let mut backend =
            LegacyV04PairBackend::for_explicit_socket(&socket.path, Duration::from_millis(30))
                .expect("explicit mock path should be accepted");
        assert_eq!(backend.health(), Err(PairBackendError::TimedOut));
        server.join().expect("mock server should finish");
    }

    #[test]
    fn drip_fed_reply_cannot_extend_the_absolute_request_deadline() {
        let socket = TestSocket::unique();
        let listener = UnixListener::bind(&socket.path).expect("bind drip-feed socket fixture");
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))
            .expect("restrict drip-feed socket fixture");
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("accept drip-feed client");
            let mut request = Vec::new();
            connection
                .read_to_end(&mut request)
                .expect("read drip-feed request");
            assert_eq!(request, b"status\n");
            for byte in b"{\"ok\":true,\"state\":\"ready\"}\n" {
                if connection.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(6));
            }
        });
        let mut backend =
            LegacyV04PairBackend::for_explicit_socket(&socket.path, Duration::from_millis(35))
                .expect("explicit drip-feed path should be accepted");

        assert_eq!(backend.health(), Err(PairBackendError::TimedOut));
        server.join().expect("drip-feed server should finish");
    }

    #[test]
    fn malformed_and_multi_reply_payloads_are_rejected() {
        for payload in [
            b"not-json\n".to_vec(),
            br#"{"ok":true,"state":"ready"}"#.to_vec(),
            b"{\"ok\":true,\"state\":\"ready\"}\n{\"ok\":true}\n".to_vec(),
        ] {
            let socket = TestSocket::unique();
            let server = spawn_mock(&socket, vec![MockReply::immediate(payload)]);
            let mut backend = client(&socket);
            assert_eq!(backend.health(), Err(PairBackendError::InvalidResponse));
            server.join().expect("mock server should finish");
        }
    }

    #[test]
    fn oversized_reply_is_rejected_before_json_parsing() {
        let socket = TestSocket::unique();
        let mut payload = vec![b'x'; MAX_LEGACY_RESPONSE_BYTES + 1];
        payload.push(b'\n');
        let server = spawn_mock(&socket, vec![MockReply::immediate(payload)]);
        let mut backend = client(&socket);
        assert_eq!(backend.health(), Err(PairBackendError::ResponseTooLarge));
        server.join().expect("mock server should finish");
    }

    #[test]
    fn status_projects_sensitive_strings_to_allowlisted_fields() {
        let marker = "sensitive-marker-must-not-escape";
        let socket = TestSocket::unique();
        let response = format!(
            "{{\"ok\":true,\"state\":\"degraded\",\"last_error\":\"{marker}\",\"aura_transport\":\"{marker}\",\"private\":\"{marker}\"}}\n"
        );
        let server = spawn_mock(&socket, vec![MockReply::immediate(response.into_bytes())]);
        let mut backend = client(&socket);
        let health = backend.health().expect("sanitized status should parse");

        assert_eq!(health.lifecycle(), PairLifecycle::Degraded);
        assert_eq!(health.level(), crate::backend::PairHealthLevel::Degraded);
        assert!(health.has_reported_error());
        assert_eq!(health.aura_transport(), AuraControlTransport::Unknown);
        assert!(!format!("{health:?}").contains(marker));
        server.join().expect("mock server should finish");
    }

    #[test]
    fn rejection_and_unknown_state_never_echo_raw_backend_text() {
        let marker = "sensitive-marker-must-not-escape";
        for response in [
            format!("{{\"ok\":false,\"state\":\"failed\",\"error\":\"{marker}\"}}\n"),
            format!("{{\"ok\":true,\"state\":\"{marker}\"}}\n"),
        ] {
            let socket = TestSocket::unique();
            let server = spawn_mock(&socket, vec![MockReply::immediate(response.into_bytes())]);
            let mut backend = client(&socket);
            let error = backend.health().expect_err("response should be rejected");
            assert!(!format!("{error:?} {error}").contains(marker));
            server.join().expect("mock server should finish");
        }
    }

    #[test]
    fn lifecycle_action_rejects_a_success_reply_with_the_wrong_post_state() {
        let socket = TestSocket::unique();
        let server = spawn_mock(
            &socket,
            vec![MockReply::immediate(b"{\"ok\":true,\"state\":\"ready\"}\n")],
        );
        let mut backend = client(&socket);
        assert_eq!(
            backend.start(),
            PairActionResult::OutcomeUnknown(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                PairBackendError::UnexpectedLifecycle,
                Some(PairLifecycle::Ready),
            ))
        );
        server.join().expect("mock server should finish");
    }
}
