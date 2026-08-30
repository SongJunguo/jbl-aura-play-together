//! Single-threaded loopback HTTP service for the embedded Play Together UI.
//!
//! The listener owns one [`crate::web::WebApp`] and therefore one
//! [`crate::web::WebActor`] for its entire lifetime.  Only the shutdown flag is
//! shared; no backend, controller or mutation handle is placed behind an
//! `Arc<Mutex<_>>`.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use openssl::rand::rand_bytes;
use zeroize::Zeroize;

use crate::web::{
    request_frame, RequestFrame, WebActor, WebApp, WebConfigError, WebSecurity, MAX_REQUEST_BYTES,
};

const READ_CHUNK_BYTES: usize = 2048;

/// Bounded service timings.  Every value is validated by `bind_with_options`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebServerOptions {
    read_deadline: Duration,
    write_deadline: Duration,
    accept_poll_interval: Duration,
}

impl WebServerOptions {
    pub const fn new(
        read_deadline: Duration,
        write_deadline: Duration,
        accept_poll_interval: Duration,
    ) -> Self {
        Self {
            read_deadline,
            write_deadline,
            accept_poll_interval,
        }
    }

    pub const fn read_deadline(self) -> Duration {
        self.read_deadline
    }

    pub const fn write_deadline(self) -> Duration {
        self.write_deadline
    }

    pub const fn accept_poll_interval(self) -> Duration {
        self.accept_poll_interval
    }

    fn is_valid(self) -> bool {
        const MAX_IO_DEADLINE: Duration = Duration::from_secs(30);
        const MAX_ACCEPT_POLL: Duration = Duration::from_millis(250);
        !self.read_deadline.is_zero()
            && self.read_deadline <= MAX_IO_DEADLINE
            && !self.write_deadline.is_zero()
            && self.write_deadline <= MAX_IO_DEADLINE
            && !self.accept_poll_interval.is_zero()
            && self.accept_poll_interval <= MAX_ACCEPT_POLL
    }
}

impl Default for WebServerOptions {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(3),
            Duration::from_secs(2),
            Duration::from_millis(20),
        )
    }
}

/// Sanitized service failures.  Raw socket addresses and operating-system
/// diagnostics are deliberately not retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebServerError {
    InvalidSecurityConfiguration(WebConfigError),
    InvalidOptions,
    RandomGenerationFailed,
    BindFailed,
    ListenerValidationFailed,
    ListenerConfigurationFailed,
    AcceptFailed,
}

impl fmt::Display for WebServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidSecurityConfiguration(_) => {
                "Web service security configuration is invalid"
            }
            Self::InvalidOptions => "Web service deadlines are invalid",
            Self::RandomGenerationFailed => "Web service CSRF generation failed",
            Self::BindFailed => "Web service loopback bind failed",
            Self::ListenerValidationFailed => "Web service listener is not trusted loopback",
            Self::ListenerConfigurationFailed => "Web service listener configuration failed",
            Self::AcceptFailed => "Web service accept failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WebServerError {}

impl From<WebConfigError> for WebServerError {
    fn from(error: WebConfigError) -> Self {
        Self::InvalidSecurityConfiguration(error)
    }
}

/// A listener-loop failure that preserves ownership of the unique actor so the
/// service host can still perform its bounded device-role shutdown.
pub struct WebServeError<A> {
    error: WebServerError,
    actor: A,
}

impl<A> WebServeError<A> {
    pub const fn error(&self) -> WebServerError {
        self.error
    }

    pub fn into_parts(self) -> (WebServerError, A) {
        (self.error, self.actor)
    }
}

impl<A> fmt::Debug for WebServeError<A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebServeError")
            .field("error", &self.error)
            .field("actor", &"retained")
            .finish()
    }
}

/// A bound but not yet running service.  Calling [`WebServer::serve`] consumes
/// it, so a second thread cannot acquire the actor by cloning this value.
pub struct WebServer<A> {
    listener: TcpListener,
    local_addr: SocketAddr,
    app: WebApp<A>,
    shutdown: Arc<AtomicBool>,
    options: WebServerOptions,
}

impl<A: WebActor> WebServer<A> {
    pub fn bind(
        actor: A,
        bind_addr: SocketAddr,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self, WebServerError> {
        Self::bind_with_options(actor, bind_addr, shutdown, WebServerOptions::default())
    }

    pub fn bind_with_options(
        actor: A,
        bind_addr: SocketAddr,
        shutdown: Arc<AtomicBool>,
        options: WebServerOptions,
    ) -> Result<Self, WebServerError> {
        if !options.is_valid() {
            return Err(WebServerError::InvalidOptions);
        }

        let mut csrf = [0u8; 32];
        rand_bytes(&mut csrf).map_err(|_| WebServerError::RandomGenerationFailed)?;
        let security_result = WebSecurity::new(bind_addr, csrf);
        csrf.zeroize();
        let security = security_result?;

        // Bind exactly the already-validated address; there is no wildcard or
        // fallback bind path.
        let listener =
            TcpListener::bind(security.bind_addr()).map_err(|_| WebServerError::BindFailed)?;
        let local_addr = listener
            .local_addr()
            .map_err(|_| WebServerError::ListenerValidationFailed)?;
        if local_addr != security.bind_addr() || !local_addr.ip().is_loopback() {
            return Err(WebServerError::ListenerValidationFailed);
        }
        listener
            .set_nonblocking(true)
            .map_err(|_| WebServerError::ListenerConfigurationFailed)?;

        Ok(Self {
            listener,
            local_addr,
            app: WebApp::new(actor, security),
            shutdown,
            options,
        })
    }

    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Run until `shutdown` becomes true, then return the uniquely owned actor.
    /// A listener-loop failure also returns that actor to the caller; it is
    /// never dropped before the service host can attempt graceful shutdown.
    pub fn serve(self) -> Result<A, WebServeError<A>> {
        self.serve_with_accept(|listener| listener.accept())
    }

    fn serve_with_accept(
        mut self,
        mut accept: impl FnMut(&TcpListener) -> io::Result<(TcpStream, SocketAddr)>,
    ) -> Result<A, WebServeError<A>> {
        while !self.shutdown.load(Ordering::Acquire) {
            match accept(&self.listener) {
                Ok((stream, accepted_peer)) => self.handle_connection(stream, accepted_peer),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(self.options.accept_poll_interval);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    return Err(WebServeError {
                        error: WebServerError::AcceptFailed,
                        actor: self.app.into_actor(),
                    });
                }
            }
        }
        Ok(self.app.into_actor())
    }

    fn handle_connection(&mut self, mut stream: TcpStream, accepted_peer: SocketAddr) {
        if stream.set_nonblocking(false).is_err() {
            close_stream(&stream);
            return;
        }
        let addresses_are_trusted = stream
            .local_addr()
            .ok()
            .zip(stream.peer_addr().ok())
            .is_some_and(|(local, peer)| {
                local == self.local_addr
                    && local.ip().is_loopback()
                    && peer == accepted_peer
                    && peer.ip().is_loopback()
            });
        if !addresses_are_trusted {
            close_stream(&stream);
            return;
        }

        match read_one_request(
            &mut stream,
            self.options.read_deadline,
            &self.shutdown,
            self.options.accept_poll_interval,
        ) {
            ReadOutcome::Complete(request) | ReadOutcome::Rejected(request) => {
                let response = self.app.handle(&request).to_http1_bytes();
                let _ = write_with_deadline(&mut stream, &response, self.options.write_deadline);
            }
            ReadOutcome::Closed => {}
        }
        close_stream(&stream);
    }
}

enum ReadOutcome {
    Complete(Vec<u8>),
    Rejected(Vec<u8>),
    Closed,
}

fn read_one_request(
    stream: &mut TcpStream,
    timeout: Duration,
    shutdown: &AtomicBool,
    stop_poll_interval: Duration,
) -> ReadOutcome {
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return ReadOutcome::Closed;
    };
    let mut request = Vec::with_capacity(READ_CHUNK_BYTES);
    let mut chunk = [0u8; READ_CHUNK_BYTES];

    loop {
        if shutdown.load(Ordering::Acquire) {
            return ReadOutcome::Closed;
        }
        match request_frame(&request) {
            RequestFrame::Complete => {
                // A second request already queued on this connection makes the
                // whole input ambiguous. Later bytes are never processed: the
                // response always advertises Connection: close.
                if has_queued_bytes(stream) {
                    request.push(0);
                    return ReadOutcome::Rejected(request);
                }
                return ReadOutcome::Complete(request);
            }
            RequestFrame::Rejected => return ReadOutcome::Rejected(request),
            RequestFrame::Incomplete => {}
        }

        let now = Instant::now();
        if now >= deadline {
            return ReadOutcome::Closed;
        }
        let remaining = deadline.saturating_duration_since(now);
        if stream
            .set_read_timeout(Some(remaining.min(stop_poll_interval)))
            .is_err()
        {
            return ReadOutcome::Closed;
        }

        let remaining_capacity = MAX_REQUEST_BYTES
            .saturating_add(1)
            .saturating_sub(request.len());
        if remaining_capacity == 0 {
            return ReadOutcome::Rejected(request);
        }
        let read_limit = remaining_capacity.min(chunk.len());
        match stream.read(&mut chunk[..read_limit]) {
            Ok(0) => return ReadOutcome::Closed,
            Ok(count) => request.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= deadline {
                    return ReadOutcome::Closed;
                }
            }
            Err(_) => return ReadOutcome::Closed,
        }
    }
}

fn has_queued_bytes(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let mut byte = [0u8; 1];
    let result = match stream.peek(&mut byte) {
        Ok(count) => count != 0,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => false,
        Err(_) => true,
    };
    // A restore failure is terminally conservative: the caller will reject
    // and then close instead of attempting another blocking read.
    if stream.set_nonblocking(false).is_err() {
        return true;
    }
    result
}

fn write_with_deadline(
    stream: &mut TcpStream,
    response: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "write deadline overflow"))?;
    let mut written = 0usize;
    while written < response.len() {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "write deadline elapsed",
            ));
        }
        stream.set_write_timeout(Some(deadline.saturating_duration_since(now)))?;
        match stream.write(&response[written..]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "closed")),
            Ok(count) => written = written.saturating_add(count),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "write deadline elapsed",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn close_stream(stream: &TcpStream) {
    let _ = stream.shutdown(Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use crate::backend::{
        AuraControlTransport, PairActionReceipt, PairActionResult, PairBackend, PairBackendError,
        PairBackendEvidence, PairBackendKind, PairHealth, PairLifecycle,
    };
    use crate::controller::{
        ControllerActionResult, ControllerStatus, PairConfigurationObservation,
        PairConfigurationProbe, PairController, PairProbeError,
    };
    use crate::journal::MemoryJournal;
    use crate::web::{RevisionConflict, WebMutation};

    use super::*;

    struct Probe;

    impl PairConfigurationProbe for Probe {
        fn pair_configuration(&mut self) -> Result<PairConfigurationObservation, PairProbeError> {
            Ok(PairConfigurationObservation::ready())
        }
    }

    struct Backend {
        health: PairLifecycle,
    }

    impl Backend {
        fn accepted(lifecycle: PairLifecycle) -> PairActionResult {
            PairActionResult::Accepted(PairActionReceipt::new(
                PairBackendKind::LegacyV04WholePair,
                lifecycle,
                PairBackendEvidence::LifecycleAcknowledgement,
                false,
            ))
        }
    }

    impl PairBackend for Backend {
        fn kind(&self) -> PairBackendKind {
            PairBackendKind::LegacyV04WholePair
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            Ok(PairHealth::new(
                PairBackendKind::LegacyV04WholePair,
                self.health,
                false,
                AuraControlTransport::Le,
            ))
        }

        fn start(&mut self) -> PairActionResult {
            self.health = PairLifecycle::Linked;
            Self::accepted(PairLifecycle::Linked)
        }

        fn stop(&mut self) -> PairActionResult {
            self.health = PairLifecycle::Ready;
            Self::accepted(PairLifecycle::Ready)
        }

        fn shutdown(&mut self) -> PairActionResult {
            self.health = PairLifecycle::ShuttingDown;
            Self::accepted(PairLifecycle::ShuttingDown)
        }
    }

    struct Actor {
        controller: PairController<Backend, Probe, MemoryJournal>,
        status_calls: usize,
        mutations: usize,
    }

    type ServerThread = thread::JoinHandle<Result<Actor, WebServeError<Actor>>>;

    impl WebActor for Actor {
        fn status(&mut self) -> ControllerStatus {
            self.status_calls += 1;
            self.controller.status()
        }

        fn mutate_if_revision(
            &mut self,
            expected_revision: u64,
            mutation: WebMutation,
        ) -> Result<ControllerActionResult, RevisionConflict> {
            if expected_revision != self.controller.status().revision() {
                return Err(RevisionConflict);
            }
            self.mutations += 1;
            Ok(match mutation {
                WebMutation::Start => self.controller.start(),
                WebMutation::Stop => self.controller.stop(),
                WebMutation::RecoverStop => self.controller.recover_stop(),
            })
        }
    }

    fn actor() -> Actor {
        Actor {
            controller: PairController::new(
                Backend {
                    health: PairLifecycle::Ready,
                },
                Probe,
            ),
            status_calls: 0,
            mutations: 0,
        }
    }

    fn available_address() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral probe");
        let address = listener.local_addr().expect("probe address");
        drop(listener);
        address
    }

    fn start_server(options: WebServerOptions) -> (SocketAddr, Arc<AtomicBool>, ServerThread) {
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = WebServer::bind_with_options(
            actor(),
            available_address(),
            Arc::clone(&shutdown),
            options,
        )
        .expect("bind server");
        let address = server.local_addr();
        let thread = thread::spawn(move || server.serve());
        (address, shutdown, thread)
    }

    fn exchange(address: SocketAddr, request: &[u8]) -> Vec<u8> {
        let mut stream =
            TcpStream::connect_timeout(&address, Duration::from_secs(1)).expect("connect server");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        stream.write_all(request).expect("write request");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        response
    }

    fn response_status(response: &[u8]) -> u16 {
        let text = std::str::from_utf8(response).expect("HTTP UTF-8");
        text.split(' ')
            .nth(1)
            .expect("status")
            .parse()
            .expect("numeric status")
    }

    fn response_header<'a>(response: &'a [u8], wanted: &str) -> Option<&'a str> {
        let text = std::str::from_utf8(response).ok()?;
        let head = text.split_once("\r\n\r\n")?.0;
        head.split("\r\n").skip(1).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(wanted)
                .then_some(value.trim_matches([' ', '\t']))
        })
    }

    fn stop_server(shutdown: Arc<AtomicBool>, thread: ServerThread) -> Actor {
        shutdown.store(true, Ordering::Release);
        thread.join().expect("server thread").expect("serve")
    }

    #[test]
    fn normal_loopback_get_and_csrf_post_reach_the_unique_actor() {
        let (address, shutdown, thread) = start_server(WebServerOptions::default());
        let host = address.to_string();
        let page = exchange(
            address,
            format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes(),
        );
        assert_eq!(response_status(&page), 200);
        let cookie = response_header(&page, "Set-Cookie")
            .expect("CSRF cookie")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_string();
        let token = cookie.split_once('=').expect("cookie value").1.to_string();

        let health = exchange(
            address,
            format!("GET /healthz HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes(),
        );
        assert_eq!(response_status(&health), 200);
        assert!(std::str::from_utf8(&health)
            .unwrap()
            .ends_with("{\"status\":\"ready\"}"));

        let status = exchange(
            address,
            format!("GET /api/status HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes(),
        );
        assert_eq!(response_status(&status), 200);
        let etag = response_header(&status, "ETag").expect("revision");
        let body = "{}";
        let start = exchange(
            address,
            format!(
                "POST /api/start HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nIf-Match: {etag}\r\nX-CSRF-Token: {token}\r\nCookie: {cookie}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
        assert_eq!(response_status(&start), 200);
        assert_eq!(response_header(&start, "Connection"), Some("close"));

        let actor = stop_server(shutdown, thread);
        assert_eq!(actor.status_calls, 1);
        assert_eq!(actor.mutations, 1);
    }

    #[test]
    fn trickle_input_cannot_extend_the_absolute_read_deadline() {
        let options = WebServerOptions::new(
            Duration::from_millis(140),
            Duration::from_secs(1),
            Duration::from_millis(5),
        );
        let (address, shutdown, thread) = start_server(options);
        let mut stream = TcpStream::connect(address).expect("connect");
        let started = Instant::now();
        for byte in b"GET /api/status HTTP/1.1\r\n" {
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(45));
        }
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        assert!(started.elapsed() < Duration::from_millis(600));
        assert!(response.is_empty());

        let actor = stop_server(shutdown, thread);
        assert_eq!(actor.status_calls, 0);
        assert_eq!(actor.mutations, 0);
    }

    #[test]
    fn oversized_declared_body_is_rejected_without_actor_access() {
        let (address, shutdown, thread) = start_server(WebServerOptions::default());
        let request =
            format!("POST /api/start HTTP/1.1\r\nHost: {address}\r\nContent-Length: 4097\r\n\r\n");
        let response = exchange(address, request.as_bytes());
        assert_eq!(response_status(&response), 413);
        let actor = stop_server(shutdown, thread);
        assert_eq!(actor.status_calls, 0);
        assert_eq!(actor.mutations, 0);
    }

    #[test]
    fn prequeued_pipeline_is_rejected_and_never_reaches_actor() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let server =
            WebServer::bind(actor(), available_address(), Arc::clone(&shutdown)).expect("bind");
        let address = server.local_addr();
        let mut stream = TcpStream::connect(address).expect("connect backlog");
        let request = format!(
            "GET /api/status HTTP/1.1\r\nHost: {address}\r\n\r\nGET /api/status HTTP/1.1\r\nHost: {address}\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .expect("queue pipeline");
        let thread = thread::spawn(move || server.serve());
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read");
        assert_eq!(response_status(&response), 400);
        assert_eq!(
            std::str::from_utf8(&response)
                .unwrap()
                .matches("HTTP/1.1")
                .count(),
            1
        );
        let actor = stop_server(shutdown, thread);
        assert_eq!(actor.status_calls, 0);
    }

    #[test]
    fn nonloopback_bind_is_rejected_before_socket_creation() {
        let result = WebServer::bind(
            actor(),
            "192.0.2.1:8096".parse().unwrap(),
            Arc::new(AtomicBool::new(false)),
        );
        assert!(matches!(
            result,
            Err(WebServerError::InvalidSecurityConfiguration(
                WebConfigError::NonLoopbackBind
            ))
        ));
    }

    #[test]
    fn atomic_shutdown_exits_idle_accept_loop_without_busy_waiting() {
        let (address, shutdown, thread) = start_server(WebServerOptions::new(
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_millis(10),
        ));
        assert!(address.ip().is_loopback());
        let (done_tx, done_rx) = mpsc::channel();
        shutdown.store(true, Ordering::Release);
        thread::spawn(move || {
            let result = thread.join().expect("server thread");
            done_tx.send(result).expect("completion");
        });
        let actor = done_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("prompt shutdown")
            .expect("serve result");
        assert_eq!(actor.mutations, 0);
    }

    #[test]
    fn accept_failure_returns_the_unique_actor_instead_of_dropping_it() {
        let server = WebServer::bind(
            actor(),
            available_address(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect("bind");
        let failure = match server.serve_with_accept(|_| {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "controlled fixture failure",
            ))
        }) {
            Err(failure) => failure,
            Ok(_) => panic!("controlled accept must fail"),
        };
        assert_eq!(failure.error(), WebServerError::AcceptFailed);
        let (error, actor) = failure.into_parts();
        assert_eq!(error, WebServerError::AcceptFailed);
        assert_eq!(actor.status_calls, 0);
        assert_eq!(actor.mutations, 0);
    }
}
