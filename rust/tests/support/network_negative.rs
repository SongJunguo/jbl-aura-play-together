use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::hash::MessageDigest;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::ssl::{SslAcceptor, SslMethod, SslVerifyMode};
use openssl::x509::{X509NameBuilder, X509};
use sha2::{Digest, Sha256};

use super::{ClientPorts, JblLanClient, MAX_RESPONSE_BYTES};
use crate::control::{PlayTogetherCommand, PlayTogetherWriteOutcome, PlayTogetherWriteResult};
use crate::eq::{EqPresetTarget, EqPresetWriteResult};
use crate::error::JblError;
use crate::media::{
    AudioSourceTarget, AudioSourceWriteResult, MediaSource, MuteTarget, MuteWriteResult,
    PlaybackTarget, PlaybackWriteResult, VolumeWriteResult,
};
use crate::model::{DeviceIdentity, SUPPORTED_JBL_MODEL};
use crate::oneos::OneOsReadCommand;
use crate::service_runtime::DirectControlLock;

const LOOPBACK: &str = "127.0.0.1";

struct IdentityMaterial {
    certificate_pem: Vec<u8>,
    private_key_pem: Vec<u8>,
    fingerprint: String,
}

fn identity_material() -> &'static IdentityMaterial {
    static MATERIAL: OnceLock<IdentityMaterial> = OnceLock::new();
    MATERIAL.get_or_init(|| {
        let rsa = Rsa::generate(2_048).expect("fixture RSA key should be generated");
        let private_key = PKey::from_rsa(rsa).expect("fixture key should parse");
        let mut name = X509NameBuilder::new().expect("fixture name builder should initialize");
        name.append_entry_by_text("CN", "local-jbl-fixture")
            .expect("fixture common name should be accepted");
        let name = name.build();

        let mut serial = BigNum::new().expect("fixture serial should initialize");
        serial
            .rand(128, MsbOption::MAYBE_ZERO, false)
            .expect("fixture serial should randomize");
        let serial = serial
            .to_asn1_integer()
            .expect("fixture serial should convert");
        let not_before = Asn1Time::days_from_now(0).expect("fixture start time should initialize");
        let not_after = Asn1Time::days_from_now(1).expect("fixture end time should initialize");

        let mut certificate = X509::builder().expect("fixture certificate should initialize");
        certificate
            .set_version(2)
            .expect("fixture certificate version should be accepted");
        certificate
            .set_serial_number(&serial)
            .expect("fixture serial should be accepted");
        certificate
            .set_subject_name(&name)
            .expect("fixture subject should be accepted");
        certificate
            .set_issuer_name(&name)
            .expect("fixture issuer should be accepted");
        certificate
            .set_pubkey(&private_key)
            .expect("fixture public key should be accepted");
        certificate
            .set_not_before(&not_before)
            .expect("fixture start time should be accepted");
        certificate
            .set_not_after(&not_after)
            .expect("fixture end time should be accepted");
        certificate
            .sign(&private_key, MessageDigest::sha256())
            .expect("fixture certificate should be signed");
        let certificate = certificate.build();
        let certificate_der = certificate
            .to_der()
            .expect("fixture certificate should encode as DER");
        let fingerprint = Sha256::digest(&certificate_der)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        IdentityMaterial {
            certificate_pem: certificate
                .to_pem()
                .expect("fixture certificate should encode as PEM"),
            private_key_pem: private_key
                .private_key_to_pem_pkcs8()
                .expect("fixture key should encode as PEM"),
            fingerprint,
        }
    })
}

struct PrivateIdentityFiles {
    directory: PathBuf,
    certificate: PathBuf,
    private_key: PathBuf,
}

impl PrivateIdentityFiles {
    fn create() -> Self {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "jbl-aura-network-negative-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("fixture directory should be created");
        let certificate = directory.join("client-cert.pem");
        let private_key = directory.join("client-key.pem");
        write_private(&certificate, &identity_material().certificate_pem);
        write_private(&private_key, &identity_material().private_key_pem);
        Self {
            directory,
            certificate,
            private_key,
        }
    }
}

impl Drop for PrivateIdentityFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn write_private(path: &Path, payload: &[u8]) {
    fs::write(path, payload).expect("private fixture should be written");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .expect("private fixture should be owner-only");
}

struct FixtureServer {
    port: u16,
    listener: TcpListener,
    worker: Option<JoinHandle<()>>,
}

impl FixtureServer {
    fn finish(mut self) {
        self.worker
            .take()
            .expect("fixture worker should exist")
            .join()
            .expect("fixture server should not panic");
    }

    fn finish_and_assert_no_extra_connection(mut self) {
        self.worker
            .take()
            .expect("fixture worker should exist")
            .join()
            .expect("fixture server should not panic");
        match self.listener.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok(_) => panic!("fixture received an unexpected extra connection"),
            Err(error) => panic!("fixture probe failed: {error}"),
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn tls_acceptor() -> SslAcceptor {
    let material = identity_material();
    let certificate =
        X509::from_pem(&material.certificate_pem).expect("fixture certificate should parse");
    let private_key = PKey::<Private>::private_key_from_pem(&material.private_key_pem)
        .expect("fixture key should parse");
    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server())
        .expect("TLS should initialize");
    builder
        .set_certificate(&certificate)
        .expect("fixture certificate should install");
    builder
        .set_private_key(&private_key)
        .expect("fixture key should install");
    builder
        .check_private_key()
        .expect("fixture certificate and key should match");
    builder
        .cert_store_mut()
        .add_cert(certificate)
        .expect("fixture client certificate should be trusted");
    builder.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
    builder.build()
}

fn spawn_tls_server<F>(handler: F) -> FixtureServer
where
    F: FnMut(&mut openssl::ssl::SslStream<TcpStream>, CapturedHttpRequest) + Send + 'static,
{
    let mut handler = handler;
    spawn_tls_server_connections(1, move |_index, stream, request| {
        handler(stream, request);
    })
}

fn spawn_tls_server_connections<F>(connection_count: usize, mut handler: F) -> FixtureServer
where
    F: FnMut(usize, &mut openssl::ssl::SslStream<TcpStream>, CapturedHttpRequest) + Send + 'static,
{
    let listener = TcpListener::bind((LOOPBACK, 0)).expect("TLS fixture should bind to loopback");
    listener
        .set_nonblocking(true)
        .expect("TLS fixture should be nonblocking");
    let port = listener
        .local_addr()
        .expect("TLS fixture address should exist")
        .port();
    let worker_listener = listener
        .try_clone()
        .expect("TLS fixture listener should clone");
    let acceptor = tls_acceptor();
    let worker = thread::spawn(move || {
        let mut held_connections = Vec::with_capacity(connection_count);
        for index in 0..connection_count {
            let stream = accept_with_deadline(&worker_listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("TLS fixture read timeout should install");
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .expect("TLS fixture write timeout should install");
            if let Ok(mut stream) = acceptor.accept(stream) {
                assert!(
                    stream.ssl().peer_certificate().is_some(),
                    "fixture must receive the configured client certificate"
                );
                let request = read_http_request(&mut stream);
                handler(index, &mut stream, request);
                // Keep earlier sockets alive while accepting later ones. This
                // makes the two-connection test fail if the client reuses a
                // keep-alive TLS stream instead of opening a fresh connection.
                held_connections.push(stream);
            }
        }
    });
    FixtureServer {
        port,
        listener,
        worker: Some(worker),
    }
}

fn accept_with_deadline(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "fixture timed out waiting for a client"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("fixture accept failed: {error}"),
        }
    }
}

fn spawn_http_server<F>(handler: F) -> FixtureServer
where
    F: FnOnce(&mut TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind((LOOPBACK, 0)).expect("HTTP fixture should bind to loopback");
    listener
        .set_nonblocking(true)
        .expect("HTTP fixture should be nonblocking");
    let port = listener
        .local_addr()
        .expect("HTTP fixture address should exist")
        .port();
    let worker_listener = listener
        .try_clone()
        .expect("HTTP fixture listener should clone");
    let worker = thread::spawn(move || {
        let mut stream = accept_with_deadline(&worker_listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("HTTP fixture read timeout should install");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("HTTP fixture write timeout should install");
        let _request = read_http_request(&mut stream);
        handler(&mut stream);
    });
    FixtureServer {
        port,
        listener,
        worker: Some(worker),
    }
}

fn spawn_http_server_connections<F>(connection_count: usize, mut handler: F) -> FixtureServer
where
    F: FnMut(usize, &mut TcpStream, CapturedHttpRequest) + Send + 'static,
{
    let listener = TcpListener::bind((LOOPBACK, 0)).expect("HTTP fixture should bind to loopback");
    listener
        .set_nonblocking(true)
        .expect("HTTP fixture should be nonblocking");
    let port = listener
        .local_addr()
        .expect("HTTP fixture address should exist")
        .port();
    let worker_listener = listener
        .try_clone()
        .expect("HTTP fixture listener should clone");
    let worker = thread::spawn(move || {
        for index in 0..connection_count {
            let mut stream = accept_with_deadline(&worker_listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("HTTP fixture read timeout should install");
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .expect("HTTP fixture write timeout should install");
            let request = read_http_request(&mut stream);
            handler(index, &mut stream, request);
        }
    });
    FixtureServer {
        port,
        listener,
        worker: Some(worker),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedHttpRequest {
    request_line: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl CapturedHttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

fn read_http_request(stream: &mut impl Read) -> CapturedHttpRequest {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let (header_end, content_length) = loop {
        let count = stream
            .read(&mut buffer)
            .expect("fixture request should be readable");
        assert!(count > 0, "fixture client closed before completing request");
        request.extend_from_slice(&buffer[..count]);
        assert!(
            request.len() <= 65_536,
            "fixture request should stay bounded"
        );
        if let Some(header_end) = find_subsequence(&request, b"\r\n\r\n") {
            let headers = std::str::from_utf8(&request[..header_end])
                .expect("fixture request headers should be UTF-8");
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            break (header_end + 4, content_length);
        }
    };
    let request_length = header_end + content_length;
    while request.len() < request_length {
        let count = stream
            .read(&mut buffer)
            .expect("fixture request body should be readable");
        assert!(count > 0, "fixture client closed before completing body");
        request.extend_from_slice(&buffer[..count]);
        assert!(
            request.len() <= 65_536,
            "fixture request should stay bounded"
        );
    }

    let headers = std::str::from_utf8(&request[..header_end - 4])
        .expect("fixture request headers should be UTF-8");
    let mut lines = headers.lines();
    let request_line = lines
        .next()
        .expect("fixture request line should exist")
        .to_string();
    let headers = lines
        .map(|line| {
            let (name, value) = line
                .split_once(':')
                .expect("fixture header should contain a colon");
            (name.to_ascii_lowercase(), value.trim().to_string())
        })
        .collect();
    CapturedHttpRequest {
        request_line,
        headers,
        body: request[header_end..request_length].to_vec(),
    }
}

fn find_subsequence(payload: &[u8], needle: &[u8]) -> Option<usize> {
    payload
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_response(stream: &mut impl Write, status: &str, headers: &str, body: &[u8]) {
    write_response_with_connection(stream, status, headers, body, "close");
}

fn write_response_with_connection(
    stream: &mut impl Write,
    status: &str,
    headers: &str,
    body: &[u8],
    connection: &str,
) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: {connection}\r\n{headers}\r\n",
        body.len()
    )
    .expect("fixture response headers should be written");
    stream
        .write_all(body)
        .expect("fixture response body should be written");
    stream.flush().expect("fixture response should flush");
}

fn test_client(
    files: &PrivateIdentityFiles,
    pin: &str,
    timeout: Duration,
    https_port: u16,
    upnp_port: u16,
) -> JblLanClient {
    JblLanClient::new_with_ports(
        LOOPBACK,
        &files.certificate,
        &files.private_key,
        pin,
        timeout,
        ClientPorts {
            https: https_port,
            upnp: upnp_port,
        },
    )
    .expect("fixture client should initialize")
}

fn assert_control_request(request: &CapturedHttpRequest, command: PlayTogetherCommand) {
    let expected_body = command.form_body().as_bytes();
    assert_eq!(request.request_line, "POST /httpapi.asp HTTP/1.1");
    assert_eq!(
        request.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(request.header("accept"), Some("application/json"));
    assert_eq!(request.header("accept-encoding"), Some("identity"));
    assert_eq!(
        request.header("content-length"),
        Some(expected_body.len().to_string().as_str())
    );
    assert_eq!(request.header("transfer-encoding"), None);
    assert_eq!(request.body, expected_body);
}

fn assert_listener_unvisited(listener: &TcpListener) {
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("redirect target unexpectedly received a connection"),
        Err(error) => panic!("redirect target probe failed: {error}"),
    }
}

#[test]
fn unrelated_permission_denied_is_not_classified_as_a_pin_mismatch() {
    let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "unrelated fixture");
    assert!(!super::error_is_peer_pin_mismatch(&error));
}

#[test]
fn enter_and_exit_each_send_one_exact_mtls_form_request() {
    for command in [PlayTogetherCommand::Enter, PlayTogetherCommand::Exit] {
        let files = PrivateIdentityFiles::create();
        let server = spawn_tls_server(move |stream, request| {
            assert_control_request(&request, command);
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                br#"{"error_code":0}"#,
            );
        });
        let client = test_client(
            &files,
            &identity_material().fingerprint,
            Duration::from_secs(1),
            server.port,
            1,
        );

        let result = client.send_play_together(command);
        assert_eq!(result.outcome(), PlayTogetherWriteOutcome::Accepted);
        assert_eq!(result.error(), None);
        assert!(matches!(result, PlayTogetherWriteResult::Accepted(_)));
        server.finish_and_assert_no_extra_connection();
    }
}

#[test]
fn consecutive_commands_use_two_distinct_tls_connections() {
    let files = PrivateIdentityFiles::create();
    let commands = [PlayTogetherCommand::Enter, PlayTogetherCommand::Exit];
    let server = spawn_tls_server_connections(2, move |index, stream, request| {
        assert_control_request(&request, commands[index]);
        write_response_with_connection(
            stream,
            "200 OK",
            "Content-Type: application/json\r\n",
            br#"{"error_code":"0"}"#,
            "keep-alive",
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        server.port,
        1,
    );

    for command in commands {
        assert_eq!(
            client.send_play_together(command).outcome(),
            PlayTogetherWriteOutcome::Accepted
        );
    }
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn disconnect_after_form_body_is_unknown_and_not_retried() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|_stream, request| {
        assert_control_request(&request, PlayTogetherCommand::Enter);
        // Drop the TLS stream only after the complete form body was observed.
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        server.port,
        1,
    );

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Enter),
        PlayTogetherWriteResult::OutcomeUnknown(JblError::NetworkUnreachable)
    );
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn write_redirect_is_not_followed_or_sent_to_its_target() {
    let files = PrivateIdentityFiles::create();
    let target = TcpListener::bind((LOOPBACK, 0)).expect("redirect target should bind");
    target
        .set_nonblocking(true)
        .expect("redirect target should be nonblocking");
    let target_port = target
        .local_addr()
        .expect("redirect target address should exist")
        .port();
    let server = spawn_tls_server(move |stream, request| {
        assert_control_request(&request, PlayTogetherCommand::Enter);
        write_response(
            stream,
            "302 Found",
            &format!("Location: https://{LOOPBACK}:{target_port}/must-not-run\r\n"),
            b"",
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        server.port,
        1,
    );

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Enter),
        PlayTogetherWriteResult::OutcomeUnknown(JblError::HttpStatus(302))
    );
    server.finish_and_assert_no_extra_connection();
    assert_listener_unvisited(&target);
}

#[test]
fn wrong_pin_rejects_write_before_any_http_request() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|_, _| panic!("wrong pin must prevent the HTTP request"));
    let mut wrong_pin = identity_material().fingerprint.clone().into_bytes();
    wrong_pin[0] = if wrong_pin[0] == b'0' { b'1' } else { b'0' };
    let wrong_pin = String::from_utf8(wrong_pin).expect("mutated pin should remain ASCII");
    let client = test_client(&files, &wrong_pin, Duration::from_secs(1), server.port, 1);

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Enter),
        PlayTogetherWriteResult::Rejected(JblError::PeerCertificateMismatch)
    );
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn nonzero_basic_response_is_an_explicit_rejection() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|stream, request| {
        assert_control_request(&request, PlayTogetherCommand::Exit);
        write_response(
            stream,
            "200 OK",
            "Content-Type: application/json\r\n",
            br#"{"error_code":7}"#,
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        server.port,
        1,
    );

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Exit),
        PlayTogetherWriteResult::Rejected(JblError::ControlCommandRejected)
    );
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn invalid_basic_response_is_outcome_unknown() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|stream, request| {
        assert_control_request(&request, PlayTogetherCommand::Enter);
        write_response(
            stream,
            "200 OK",
            "Content-Type: application/json\r\n",
            br#"{}"#,
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        server.port,
        1,
    );

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Enter),
        PlayTogetherWriteResult::OutcomeUnknown(JblError::BasicResponseCodeMissing)
    );
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn basic_zero_on_non_200_success_status_is_outcome_unknown() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|stream, request| {
        assert_control_request(&request, PlayTogetherCommand::Enter);
        write_response(
            stream,
            "201 Created",
            "Content-Type: application/json\r\n",
            br#"{"error_code":0}"#,
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        server.port,
        1,
    );

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Enter),
        PlayTogetherWriteResult::OutcomeUnknown(JblError::HttpStatus(201))
    );
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn nonzero_basic_response_on_http_500_is_outcome_unknown() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|stream, request| {
        assert_control_request(&request, PlayTogetherCommand::Exit);
        write_response(
            stream,
            "500 Internal Server Error",
            "Content-Type: application/json\r\n",
            br#"{"error_code":7}"#,
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        server.port,
        1,
    );

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Exit),
        PlayTogetherWriteResult::OutcomeUnknown(JblError::HttpStatus(500))
    );
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn oversized_write_response_is_outcome_unknown() {
    let files = PrivateIdentityFiles::create();
    let oversized = vec![b'x'; (MAX_RESPONSE_BYTES + 1) as usize];
    let server = spawn_tls_server(move |stream, request| {
        assert_control_request(&request, PlayTogetherCommand::Enter);
        write_response(stream, "200 OK", "", &oversized);
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(2),
        server.port,
        1,
    );

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Enter),
        PlayTogetherWriteResult::OutcomeUnknown(JblError::ResponseTooLarge)
    );
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn write_response_timeout_is_outcome_unknown() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|stream, request| {
        assert_control_request(&request, PlayTogetherCommand::Exit);
        thread::sleep(Duration::from_millis(350));
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{\"error_code\":0}",
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_millis(100),
        server.port,
        1,
    );

    assert_eq!(
        client.send_play_together(PlayTogetherCommand::Exit),
        PlayTogetherWriteResult::OutcomeUnknown(JblError::NetworkUnreachable)
    );
    server.finish_and_assert_no_extra_connection();
}

#[test]
fn wrong_tls_pin_is_rejected_during_local_handshake() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|_, _| panic!("pin mismatch must stop before an HTTP request"));
    let mut wrong_pin = identity_material().fingerprint.clone().into_bytes();
    wrong_pin[0] = if wrong_pin[0] == b'0' { b'1' } else { b'0' };
    let wrong_pin = String::from_utf8(wrong_pin).expect("mutated pin should remain ASCII");
    let client = test_client(&files, &wrong_pin, Duration::from_secs(1), server.port, 1);

    assert_eq!(
        client.get_json(OneOsReadCommand::DeviceInfo).unwrap_err(),
        JblError::PeerCertificateMismatch
    );
    server.finish();
}

#[test]
fn redirect_response_is_returned_instead_of_followed() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|stream, _request| {
        write_response(
            stream,
            "302 Found",
            "Location: https://127.0.0.1:1/must-not-be-followed\r\n",
            b"",
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        server.port,
        1,
    );

    assert_eq!(
        client.get_json(OneOsReadCommand::DeviceInfo).unwrap_err(),
        JblError::HttpStatus(302)
    );
    server.finish();
}

#[test]
fn slow_response_headers_hit_the_bounded_timeout() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|stream, _request| {
        thread::sleep(Duration::from_millis(350));
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_millis(100),
        server.port,
        1,
    );
    let started = Instant::now();

    assert_eq!(
        client.get_json(OneOsReadCommand::DeviceInfo).unwrap_err(),
        JblError::NetworkUnreachable
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "slow fixture must not defeat the request timeout"
    );
    server.finish();
}

#[test]
fn drip_fed_body_cannot_extend_the_total_timeout() {
    let files = PrivateIdentityFiles::create();
    let server = spawn_tls_server(|stream, _request| {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n")
            .expect("fixture headers should be written");
        stream.flush().expect("fixture headers should flush");
        for byte in b"12345" {
            thread::sleep(Duration::from_millis(70));
            if stream.write_all(&[*byte]).is_err() || stream.flush().is_err() {
                break;
            }
        }
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_millis(180),
        server.port,
        1,
    );
    let started = Instant::now();

    assert_eq!(
        client.get_json(OneOsReadCommand::DeviceInfo).unwrap_err(),
        JblError::NetworkUnreachable
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "short per-byte gaps must not reset the overall request deadline"
    );
    server.finish();
}

#[test]
fn response_larger_than_one_mib_is_rejected_before_json_parsing() {
    let files = PrivateIdentityFiles::create();
    let oversized = vec![b'x'; (MAX_RESPONSE_BYTES + 1) as usize];
    let server = spawn_tls_server(move |stream, _request| {
        write_response(stream, "200 OK", "", &oversized);
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(2),
        server.port,
        1,
    );

    assert_eq!(
        client.get_json(OneOsReadCommand::DeviceInfo).unwrap_err(),
        JblError::ResponseTooLarge
    );
    server.finish();
}

#[test]
fn upnp_model_mismatch_is_rejected() {
    let files = PrivateIdentityFiles::create();
    let tls_server = spawn_tls_server(|stream, _request| {
        write_response(
            stream,
            "200 OK",
            "Content-Type: application/json\r\n",
            br#"{"error_code":"0","device_info":{}}"#,
        );
    });
    let body = concat!(
        "<?xml version=\"1.0\"?>",
        "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
        "<s:Body><u:GetControlDeviceInfoResponse ",
        "xmlns:u=\"urn:schemas-upnp-org:service:RenderingControl:1\">",
        "<Status>{\"hm_product_name\":\"Unexpected Fixture\"}</Status>",
        "</u:GetControlDeviceInfoResponse></s:Body></s:Envelope>"
    );
    let server = spawn_http_server(move |stream| {
        write_response(
            stream,
            "200 OK",
            "Content-Type: text/xml\r\n",
            body.as_bytes(),
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        server.port,
    );

    assert_eq!(
        client
            .sanitized_status(
                SUPPORTED_JBL_MODEL,
                DeviceIdentity::parse("02:00:00:00:00:01").expect("JBL placeholder should parse"),
                DeviceIdentity::parse("02:00:00:00:00:02").expect("Aura placeholder should parse"),
            )
            .unwrap_err(),
        JblError::UnexpectedDeviceModel
    );
    tls_server.finish();
    server.finish();
}

fn upnp_model_body(model: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\"?>",
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
            "<s:Body><u:GetControlDeviceInfoResponse ",
            "xmlns:u=\"urn:schemas-upnp-org:service:RenderingControl:1\">",
            "<Status>{{\"hm_product_name\":\"{}\"}}</Status>",
            "</u:GetControlDeviceInfoResponse></s:Body></s:Envelope>"
        ),
        model
    )
}

fn upnp_action_failed_fault() -> &'static [u8] {
    br#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>501</errorCode><errorDescription>Action Failed</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#
}

fn upnp_info_body(volume: Option<u8>) -> String {
    upnp_info_body_with_mute(volume, Some(false))
}

fn upnp_info_body_with_mute(volume: Option<u8>, muted: Option<bool>) -> String {
    upnp_info_body_full(volume, muted, "PAUSED_PLAYBACK", "OK")
}

fn upnp_info_body_full(
    volume: Option<u8>,
    muted: Option<bool>,
    state: &str,
    status: &str,
) -> String {
    let volume = volume.map_or_else(String::new, |value| {
        format!("<CurrentVolume>{value}</CurrentVolume>")
    });
    let muted = muted.map_or_else(String::new, |value| {
        format!("<CurrentMute>{}</CurrentMute>", u8::from(value))
    });
    format!(
        concat!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
            "<s:Body><u:GetInfoExResponse ",
            "xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">",
            "<CurrentTransportState>{}</CurrentTransportState>",
            "<CurrentTransportStatus>{}</CurrentTransportStatus>",
            "{}{}",
            "</u:GetInfoExResponse></s:Body></s:Envelope>"
        ),
        state, status, volume, muted
    )
}

fn assert_model_request(request: &CapturedHttpRequest) {
    assert_eq!(
        request.request_line,
        "POST /upnp/control/rendercontrol1 HTTP/1.1"
    );
    assert!(request
        .header("soapaction")
        .is_some_and(|value| value.contains("#GetControlDeviceInfo")));
}

fn assert_info_request(request: &CapturedHttpRequest) {
    assert!(request.request_line.contains("rendertransport1"));
    assert!(request
        .header("soapaction")
        .is_some_and(|value| value.contains("#GetInfoEx")));
}

fn assert_set_volume_request(request: &CapturedHttpRequest, volume: u8) {
    assert!(request.request_line.contains("rendercontrol1"));
    assert!(request
        .header("soapaction")
        .is_some_and(|value| value.contains("#SetVolume")));
    let body = std::str::from_utf8(&request.body).expect("fixture body is UTF-8");
    assert!(body.contains("<Channel>Single</Channel>"));
    assert!(body.contains(&format!("<DesiredVolume>{volume}</DesiredVolume>")));
}

fn assert_set_mute_request(request: &CapturedHttpRequest, target: MuteTarget) {
    assert!(request.request_line.contains("rendercontrol1"));
    assert!(request
        .header("soapaction")
        .is_some_and(|value| value.contains("#SetMute")));
    let body = std::str::from_utf8(&request.body).expect("fixture body is UTF-8");
    assert!(body.contains("<Channel>Master</Channel>"));
    let desired = match target {
        MuteTarget::On => 1,
        MuteTarget::Off => 0,
    };
    assert!(body.contains(&format!("<DesiredMute>{desired}</DesiredMute>")));
    assert!(!body.contains("Toggle"));
}

fn assert_playback_request(request: &CapturedHttpRequest, target: PlaybackTarget) {
    assert!(request.request_line.contains("rendertransport1"));
    let action = match target {
        PlaybackTarget::Play => "#Play",
        PlaybackTarget::Pause => "#Pause",
    };
    assert!(request
        .header("soapaction")
        .is_some_and(|value| value.contains(action)));
    let body = std::str::from_utf8(&request.body).expect("fixture body is UTF-8");
    assert!(body.contains("<InstanceID>0</InstanceID>"));
    assert_eq!(
        body.contains("<Speed>1</Speed>"),
        target == PlaybackTarget::Play
    );
    for forbidden in ["#Stop", "#Next", "#Previous"] {
        assert!(!request
            .header("soapaction")
            .is_some_and(|value| value.contains(forbidden)));
    }
}

fn supported_device_info_tls_server() -> FixtureServer {
    supported_device_info_tls_server_connections(1)
}

fn supported_device_info_tls_server_connections(connection_count: usize) -> FixtureServer {
    spawn_tls_server_connections(connection_count, |_index, stream, request| {
        assert!(request
            .request_line
            .contains("/httpapi.asp?command=getDeviceInfo"));
        write_response(
            stream,
            "200 OK",
            "Content-Type: application/json\r\n",
            br#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#,
        );
    })
}

fn unvisited_listener() -> TcpListener {
    let listener = TcpListener::bind((LOOPBACK, 0)).expect("negative fixture should bind");
    listener
        .set_nonblocking(true)
        .expect("negative fixture should be nonblocking");
    listener
}

#[test]
fn volume_write_is_single_shot_and_requires_independent_readback() {
    let files = PrivateIdentityFiles::create();
    let tls_server = supported_device_info_tls_server_connections(2);
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(4, move |index, stream, request| match index {
        0 => {
            assert_model_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                model_body.as_bytes(),
            );
        }
        1 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(21)).as_bytes(),
            );
        }
        2 => {
            assert_set_volume_request(&request, 9);
            write_response(stream, "200 OK", "Content-Type: text/xml\r\n", b"");
        }
        3 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(9)).as_bytes(),
            );
        }
        _ => unreachable!("fixture accepts exactly four UPnP requests"),
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );

    let mut lock = DirectControlLock::for_protocol_fixture();
    let result = client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 9);
    assert!(matches!(
        result,
        VolumeWriteResult::Applied(ref playback) if playback.volume == Some(9)
    ));
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
}

#[test]
fn volume_above_safety_limit_is_rejected_before_any_network() {
    let files = PrivateIdentityFiles::create();
    let https = unvisited_listener();
    let upnp = unvisited_listener();
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        https.local_addr().unwrap().port(),
        upnp.local_addr().unwrap().port(),
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert_eq!(
        client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 10),
        VolumeWriteResult::RejectedBeforeSend(JblError::VolumeSafetyLimitExceeded)
    );
    assert_listener_unvisited(&https);
    assert_listener_unvisited(&upnp);
}

#[test]
fn volume_pin_failure_reaches_no_upnp_surface() {
    let files = PrivateIdentityFiles::create();
    let tls_server =
        spawn_tls_server(|_, _| panic!("pin mismatch must stop before an HTTP request"));
    let upnp = unvisited_listener();
    let mut wrong_pin = identity_material().fingerprint.clone().into_bytes();
    wrong_pin[0] = if wrong_pin[0] == b'0' { b'1' } else { b'0' };
    let wrong_pin = String::from_utf8(wrong_pin).expect("mutated pin should remain ASCII");
    let client = test_client(
        &files,
        &wrong_pin,
        Duration::from_secs(1),
        tls_server.port,
        upnp.local_addr().unwrap().port(),
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert_eq!(
        client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 9),
        VolumeWriteResult::RejectedBeforeSend(JblError::PeerCertificateMismatch)
    );
    tls_server.finish_and_assert_no_extra_connection();
    assert_listener_unvisited(&upnp);
}

#[test]
fn volume_model_mismatch_stops_before_snapshot_or_write() {
    let files = PrivateIdentityFiles::create();
    let tls_server = supported_device_info_tls_server();
    let model_body = upnp_model_body("not-the-supported-model");
    let upnp_server = spawn_http_server_connections(1, move |_index, stream, request| {
        assert_model_request(&request);
        write_response(
            stream,
            "200 OK",
            "Content-Type: text/xml\r\n",
            model_body.as_bytes(),
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert_eq!(
        client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 9),
        VolumeWriteResult::RejectedBeforeSend(JblError::UnexpectedDeviceModel)
    );
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
}

#[test]
fn missing_before_volume_stops_before_mutation() {
    let files = PrivateIdentityFiles::create();
    let tls_server = supported_device_info_tls_server();
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(2, move |index, stream, request| {
        let body = match index {
            0 => {
                assert_model_request(&request);
                model_body.clone()
            }
            1 => {
                assert_info_request(&request);
                upnp_info_body(None)
            }
            _ => unreachable!(),
        };
        write_response(
            stream,
            "200 OK",
            "Content-Type: text/xml\r\n",
            body.as_bytes(),
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert_eq!(
        client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 9),
        VolumeWriteResult::RejectedBeforeSend(JblError::MediaVolumeMissing)
    );
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
}

#[test]
fn already_target_volume_sends_no_mutation() {
    let files = PrivateIdentityFiles::create();
    let tls_server = supported_device_info_tls_server();
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(2, move |index, stream, request| {
        let body = match index {
            0 => {
                assert_model_request(&request);
                model_body.clone()
            }
            1 => {
                assert_info_request(&request);
                upnp_info_body(Some(9))
            }
            _ => unreachable!(),
        };
        write_response(
            stream,
            "200 OK",
            "Content-Type: text/xml\r\n",
            body.as_bytes(),
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert!(matches!(
        client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 9),
        VolumeWriteResult::AlreadyAtTarget(ref playback) if playback.volume == Some(9)
    ));
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
}

#[test]
fn lost_volume_reply_is_not_retried_or_promoted_to_success() {
    let files = PrivateIdentityFiles::create();
    let tls_server = supported_device_info_tls_server_connections(2);
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(4, move |index, stream, request| match index {
        0 => {
            assert_model_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                model_body.as_bytes(),
            );
        }
        1 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(8)).as_bytes(),
            );
        }
        2 => {
            assert_set_volume_request(&request, 9);
            // Drop only after the complete mutation body has been captured.
        }
        3 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(9)).as_bytes(),
            );
        }
        _ => unreachable!(),
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert!(matches!(
        client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 9),
        VolumeWriteResult::TargetObservedAfterUnknownWrite(ref playback)
            if playback.volume == Some(9)
    ));
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
}

#[test]
fn successful_volume_transport_with_conflicting_readback_fails_postcondition() {
    let files = PrivateIdentityFiles::create();
    let tls_server = supported_device_info_tls_server_connections(2);
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(4, move |index, stream, request| match index {
        0 => {
            assert_model_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                model_body.as_bytes(),
            );
        }
        1 | 3 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(8)).as_bytes(),
            );
        }
        2 => {
            assert_set_volume_request(&request, 9);
            write_response(stream, "200 OK", "Content-Type: text/xml\r\n", b"");
        }
        _ => unreachable!(),
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert!(matches!(
        client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 9),
        VolumeWriteResult::PostconditionFailed(ref playback) if playback.volume == Some(8)
    ));
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
}

#[test]
fn post_write_pinned_identity_failure_keeps_volume_outcome_unknown() {
    let files = PrivateIdentityFiles::create();
    let tls_server = spawn_tls_server_connections(2, |index, stream, request| {
        assert!(request
            .request_line
            .contains("/httpapi.asp?command=getDeviceInfo"));
        let body: &[u8] = if index == 0 {
            br#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#
        } else {
            br#"{"error_code":"0"}"#
        };
        write_response(stream, "200 OK", "Content-Type: application/json\r\n", body);
    });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(4, move |index, stream, request| match index {
        0 => {
            assert_model_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                model_body.as_bytes(),
            );
        }
        1 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(8)).as_bytes(),
            );
        }
        2 => {
            assert_set_volume_request(&request, 9);
            write_response(stream, "200 OK", "Content-Type: text/xml\r\n", b"");
        }
        3 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(9)).as_bytes(),
            );
        }
        _ => unreachable!(),
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert_eq!(
        client.set_volume(&mut lock, SUPPORTED_JBL_MODEL, 9),
        VolumeWriteResult::OutcomeUnknown(JblError::DeviceInfoMissing)
    );
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
}

fn run_mute_fixture(
    before: Option<bool>,
    before_volume: Option<u8>,
    target: MuteTarget,
    write_returns_http: Option<bool>,
    after: Option<bool>,
) -> MuteWriteResult {
    let files = PrivateIdentityFiles::create();
    let sends_mutation = before.is_some_and(|value| value != target.desired())
        && before_volume.is_some_and(|volume| volume <= crate::media::MAX_SAFE_DIRECT_VOLUME);
    assert_eq!(sends_mutation, write_returns_http.is_some());
    let tls_server =
        supported_device_info_tls_server_connections(if sends_mutation { 2 } else { 1 });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(
        if sends_mutation { 4 } else { 2 },
        move |index, stream, request| match index {
            0 => {
                assert_model_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    model_body.as_bytes(),
                );
            }
            1 => {
                assert_info_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    upnp_info_body_with_mute(before_volume, before).as_bytes(),
                );
            }
            2 => {
                assert_set_mute_request(&request, target);
                if write_returns_http == Some(true) {
                    write_response(stream, "200 OK", "Content-Type: text/xml\r\n", b"");
                }
            }
            3 => {
                assert_info_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    upnp_info_body_with_mute(before_volume, after).as_bytes(),
                );
            }
            _ => unreachable!(),
        },
    );
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();
    let result = client.set_mute(&mut lock, SUPPORTED_JBL_MODEL, target);
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
    result
}

#[test]
fn mute_write_applies_once_and_requires_independent_readback() {
    assert!(matches!(
        run_mute_fixture(Some(false), Some(9), MuteTarget::On, Some(true), Some(true)),
        MuteWriteResult::Applied(ref playback) if playback.muted == Some(true)
    ));
}

#[test]
fn mute_already_at_target_sends_no_mutation() {
    assert!(matches!(
        run_mute_fixture(Some(true), Some(9), MuteTarget::On, None, None),
        MuteWriteResult::AlreadyAtTarget(ref playback) if playback.muted == Some(true)
    ));
}

#[test]
fn mute_prewrite_network_failure_retries_only_before_any_mutation() {
    let files = PrivateIdentityFiles::create();
    let tls_server = spawn_tls_server_connections(2, |index, stream, request| {
        assert!(request
            .request_line
            .contains("/httpapi.asp?command=getDeviceInfo"));
        if index == 1 {
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                br#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#,
            );
        }
    });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(2, move |index, stream, request| {
        let body = match index {
            0 => {
                assert_model_request(&request);
                model_body.clone()
            }
            1 => {
                assert_info_request(&request);
                upnp_info_body_with_mute(Some(9), Some(false))
            }
            _ => unreachable!(),
        };
        write_response(
            stream,
            "200 OK",
            "Content-Type: text/xml\r\n",
            body.as_bytes(),
        );
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();

    assert!(matches!(
        client.set_mute(&mut lock, SUPPORTED_JBL_MODEL, MuteTarget::Off),
        MuteWriteResult::AlreadyAtTarget(ref playback) if playback.muted == Some(false)
    ));
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
}

#[test]
fn missing_before_mute_stops_before_mutation() {
    assert_eq!(
        run_mute_fixture(None, Some(9), MuteTarget::On, None, None),
        MuteWriteResult::RejectedBeforeSend(JblError::MediaMuteMissing)
    );
}

#[test]
fn mute_state_change_requires_known_safe_volume_before_mutation() {
    assert_eq!(
        run_mute_fixture(Some(false), None, MuteTarget::On, None, None),
        MuteWriteResult::RejectedBeforeSend(JblError::MediaVolumeMissing)
    );
    assert_eq!(
        run_mute_fixture(Some(false), Some(10), MuteTarget::On, None, None),
        MuteWriteResult::RejectedBeforeSend(JblError::VolumeSafetyLimitExceeded)
    );
}

#[test]
fn lost_mute_reply_is_not_retried_or_promoted_to_success() {
    assert!(matches!(
        run_mute_fixture(Some(false), Some(9), MuteTarget::On, Some(false), Some(true)),
        MuteWriteResult::TargetObservedAfterUnknownWrite(ref playback)
            if playback.muted == Some(true)
    ));
}

#[test]
fn successful_mute_transport_with_conflicting_readback_fails_postcondition() {
    assert!(matches!(
        run_mute_fixture(Some(false), Some(9), MuteTarget::On, Some(true), Some(false)),
        MuteWriteResult::PostconditionFailed(ref playback) if playback.muted == Some(false)
    ));
}

#[derive(Clone, Copy)]
enum PlaybackMutationReply {
    Success,
    Lost,
    ActionFailed,
}

fn run_playback_mutation_fixture(
    target: PlaybackTarget,
    before_state: &'static str,
    volume: Option<u8>,
    reply: PlaybackMutationReply,
    after_source: &'static str,
    after_state: &'static str,
) -> PlaybackWriteResult {
    let files = PrivateIdentityFiles::create();
    let tls_server = spawn_tls_server_connections(4, move |index, stream, request| match index {
        0 | 3 => {
            assert!(request.request_line.contains("command=getDeviceInfo"));
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                br#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#,
            );
        }
        1 | 2 => {
            assert!(request.request_line.contains("command=getMediaSource"));
            let source = if index == 1 { "BT" } else { after_source };
            let body = format!(r#"{{"error_code":0,"media_source":"{source}"}}"#);
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                body.as_bytes(),
            );
        }
        _ => unreachable!(),
    });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(4, move |index, stream, request| match index {
        0 => {
            assert_model_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                model_body.as_bytes(),
            );
        }
        1 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body_full(volume, Some(false), before_state, "OK").as_bytes(),
            );
        }
        2 => {
            assert_playback_request(&request, target);
            match reply {
                PlaybackMutationReply::Success => {
                    write_response(stream, "200 OK", "Content-Type: text/xml\r\n", b"");
                }
                PlaybackMutationReply::Lost => {}
                PlaybackMutationReply::ActionFailed => write_response(
                    stream,
                    "500 Internal Server Error",
                    "Content-Type: text/xml\r\n",
                    upnp_action_failed_fault(),
                ),
            }
        }
        3 => {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body_full(volume, Some(false), after_state, "OK").as_bytes(),
            );
        }
        _ => unreachable!(),
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();
    let result = client.set_playback(&mut lock, SUPPORTED_JBL_MODEL, target);
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
    result
}

fn run_playback_prewrite_fixture(
    source: &'static str,
    state: &'static str,
    status: &'static str,
    volume: Option<u8>,
    target: PlaybackTarget,
) -> PlaybackWriteResult {
    let files = PrivateIdentityFiles::create();
    let tls_server = spawn_tls_server_connections(2, move |index, stream, request| {
        if index == 0 {
            assert!(request.request_line.contains("command=getDeviceInfo"));
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                br#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#,
            );
        } else {
            assert!(request.request_line.contains("command=getMediaSource"));
            let body = format!(r#"{{"error_code":0,"media_source":"{source}"}}"#);
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                body.as_bytes(),
            );
        }
    });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(
        if source == "BT" { 2 } else { 1 },
        move |index, stream, request| {
            if index == 0 {
                assert_model_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    model_body.as_bytes(),
                );
            } else {
                assert_info_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    upnp_info_body_full(volume, Some(false), state, status).as_bytes(),
                );
            }
        },
    );
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();
    let result = client.set_playback(&mut lock, SUPPORTED_JBL_MODEL, target);
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
    result
}

#[test]
fn bluetooth_play_and_pause_each_apply_one_exact_write() {
    assert!(matches!(
        run_playback_mutation_fixture(
            PlaybackTarget::Play,
            "PAUSED_PLAYBACK",
            Some(9),
            PlaybackMutationReply::Success,
            "BT",
            "PLAYING",
        ),
        PlaybackWriteResult::Applied(_)
    ));
    assert!(matches!(
        run_playback_mutation_fixture(
            PlaybackTarget::Pause,
            "PLAYING",
            Some(9),
            PlaybackMutationReply::Success,
            "BT",
            "PAUSED_PLAYBACK",
        ),
        PlaybackWriteResult::Applied(_)
    ));
}

#[test]
fn lost_playback_reply_is_not_retried_or_promoted() {
    assert!(matches!(
        run_playback_mutation_fixture(
            PlaybackTarget::Play,
            "STOPPED",
            Some(9),
            PlaybackMutationReply::Lost,
            "BT",
            "PLAYING",
        ),
        PlaybackWriteResult::TargetObservedAfterUnknownWrite(_)
    ));
}

#[test]
fn successful_playback_write_with_source_change_fails_postcondition() {
    assert!(matches!(
        run_playback_mutation_fixture(
            PlaybackTarget::Pause,
            "PLAYING",
            Some(9),
            PlaybackMutationReply::Success,
            "AP2",
            "PAUSED_PLAYBACK",
        ),
        PlaybackWriteResult::PostconditionFailed(ref media)
            if media.source == crate::media::MediaSource::AirPlay2
    ));
}

#[test]
fn playback_readback_conflict_is_not_success() {
    assert!(matches!(
        run_playback_mutation_fixture(
            PlaybackTarget::Play,
            "STOPPED",
            Some(9),
            PlaybackMutationReply::Success,
            "BT",
            "STOPPED",
        ),
        PlaybackWriteResult::PostconditionFailed(_)
    ));
}

#[test]
fn exact_501_action_fault_is_one_write_and_device_rejection() {
    assert!(matches!(
        run_playback_mutation_fixture(
            PlaybackTarget::Play,
            "STOPPED",
            Some(9),
            PlaybackMutationReply::ActionFailed,
            "BT",
            "STOPPED",
        ),
        PlaybackWriteResult::RejectedByDevice(ref media)
            if media.playback.state == crate::media::TransportState::Stopped
    ));
}

#[test]
fn playback_already_target_sends_no_mutation() {
    assert!(matches!(
        run_playback_prewrite_fixture("BT", "PLAYING", "OK", Some(9), PlaybackTarget::Play),
        PlaybackWriteResult::AlreadyAtTarget(_)
    ));
}

#[test]
fn playback_rejects_non_bluetooth_and_unknown_sources_before_write() {
    for source in ["AP2", "PRIVATE"] {
        assert_eq!(
            run_playback_prewrite_fixture(
                source,
                "PAUSED_PLAYBACK",
                "OK",
                Some(9),
                PlaybackTarget::Play,
            ),
            PlaybackWriteResult::RejectedBeforeSend(JblError::UnsupportedMediaSource)
        );
    }
}

#[test]
fn play_rejects_missing_or_unsafe_volume_and_invalid_state() {
    assert_eq!(
        run_playback_prewrite_fixture("BT", "PAUSED_PLAYBACK", "OK", None, PlaybackTarget::Play),
        PlaybackWriteResult::RejectedBeforeSend(JblError::MediaVolumeMissing)
    );
    assert_eq!(
        run_playback_prewrite_fixture(
            "BT",
            "PAUSED_PLAYBACK",
            "OK",
            Some(10),
            PlaybackTarget::Play
        ),
        PlaybackWriteResult::RejectedBeforeSend(JblError::VolumeSafetyLimitExceeded)
    );
    assert_eq!(
        run_playback_prewrite_fixture(
            "BT",
            "NO_MEDIA_PRESENT",
            "OK",
            Some(9),
            PlaybackTarget::Play
        ),
        PlaybackWriteResult::RejectedBeforeSend(JblError::PlaybackPreconditionFailed)
    );
}

#[test]
fn lost_playback_reply_plus_source_change_stays_unknown() {
    assert_eq!(
        run_playback_mutation_fixture(
            PlaybackTarget::Pause,
            "PLAYING",
            Some(9),
            PlaybackMutationReply::Lost,
            "AP2",
            "PAUSED_PLAYBACK",
        ),
        PlaybackWriteResult::OutcomeUnknown(JblError::MediaSourceChanged)
    );
}

#[derive(Clone, Copy)]
enum SourceMutationReply {
    Success,
    Rejected,
    Lost,
}

fn assert_source_mutation_request(request: &CapturedHttpRequest, target: AudioSourceTarget) {
    assert_eq!(request.header("content-type"), None);
    assert_eq!(request.header("accept"), Some("application/json"));
    assert_eq!(request.header("accept-encoding"), Some("identity"));
    assert_eq!(request.header("transfer-encoding"), None);
    let expected = match target {
        AudioSourceTarget::Bluetooth => {
            br#"command=setMediaSource&payload={"media_source":"BT"}"#.as_slice()
        }
        AudioSourceTarget::AuxIn => {
            br#"command=setMediaSource&payload={"media_source":"AUX"}"#.as_slice()
        }
        AudioSourceTarget::UsbPlayback => {
            br#"command=setMediaSource&payload={"media_source":"USB"}"#.as_slice()
        }
    };
    assert_eq!(request.body, expected);
    assert_eq!(
        request.header("content-length"),
        Some(expected.len().to_string().as_str())
    );
    assert!(!request.body.contains(&b'%'));
    assert!(!request.body.windows(3).any(|window| window == b"%7B"));
    assert_eq!(request.request_line, "POST /httpapi.asp? HTTP/1.1");
}

fn source_list_with_target_and_unknown() -> &'static str {
    r#"{"error_code":0,"audiosource_info":{"support_sources":[{"source":"BT","type":1},{"source":"AUX","type":2},{"source":"USB","type":3},{"source":"PRIVATE","type":9}]}}"#
}

fn run_source_mutation_fixture(
    current: &'static str,
    target: AudioSourceTarget,
    reply: SourceMutationReply,
    after: &'static str,
    post_identity_valid: bool,
) -> AudioSourceWriteResult {
    let files = PrivateIdentityFiles::create();
    let tls_server = spawn_tls_server_connections(6, move |index, stream, request| match index {
        0 | 5 => {
            assert!(request.request_line.contains("command=getDeviceInfo"));
            let body: &[u8] = if index == 5 && !post_identity_valid {
                br#"{"error_code":0}"#
            } else {
                br#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#
            };
            write_response(stream, "200 OK", "Content-Type: application/json\r\n", body);
        }
        1 | 4 => {
            assert!(request.request_line.contains("command=getMediaSource"));
            let source = if index == 1 { current } else { after };
            let body = format!(r#"{{"error_code":0,"media_source":"{source}"}}"#);
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                body.as_bytes(),
            );
        }
        2 => {
            assert!(request
                .request_line
                .contains("command=getDeviceAudioSourceList"));
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                source_list_with_target_and_unknown().as_bytes(),
            );
        }
        3 => {
            assert_source_mutation_request(&request, target);
            match reply {
                SourceMutationReply::Success => write_response(
                    stream,
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    br#"{"error_code":0}"#,
                ),
                SourceMutationReply::Rejected => write_response(
                    stream,
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    br#"{"error_code":7}"#,
                ),
                SourceMutationReply::Lost => {}
            }
        }
        _ => unreachable!(),
    });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(2, move |index, stream, request| {
        if index == 0 {
            assert_model_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                model_body.as_bytes(),
            );
        } else {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(9)).as_bytes(),
            );
        }
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();
    let result = client.set_audio_source(&mut lock, SUPPORTED_JBL_MODEL, target);
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
    result
}

fn run_source_prewrite_fixture(
    current: &'static str,
    source_list: &'static str,
    volume_read: Option<Option<u8>>,
    target: AudioSourceTarget,
) -> AudioSourceWriteResult {
    let files = PrivateIdentityFiles::create();
    let tls_server = spawn_tls_server_connections(3, move |index, stream, request| {
        let body = match index {
            0 => {
                assert!(request.request_line.contains("command=getDeviceInfo"));
                r#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#.to_string()
            }
            1 => {
                assert!(request.request_line.contains("command=getMediaSource"));
                format!(r#"{{"error_code":0,"media_source":"{current}"}}"#)
            }
            2 => {
                assert!(request
                    .request_line
                    .contains("command=getDeviceAudioSourceList"));
                source_list.to_string()
            }
            _ => unreachable!(),
        };
        write_response(
            stream,
            "200 OK",
            "Content-Type: application/json\r\n",
            body.as_bytes(),
        );
    });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(
        1 + usize::from(volume_read.is_some()),
        move |index, stream, request| {
            if index == 0 {
                assert_model_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    model_body.as_bytes(),
                );
            } else {
                assert_info_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    upnp_info_body(volume_read.flatten()).as_bytes(),
                );
            }
        },
    );
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();
    let result = client.set_audio_source(&mut lock, SUPPORTED_JBL_MODEL, target);
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
    result
}

#[test]
fn source_aux_and_restore_bluetooth_use_exact_raw_shapes() {
    assert_eq!(
        run_source_mutation_fixture(
            "BT",
            AudioSourceTarget::AuxIn,
            SourceMutationReply::Success,
            "AUX",
            true,
        ),
        AudioSourceWriteResult::Applied(MediaSource::AuxIn)
    );
    assert_eq!(
        run_source_mutation_fixture(
            "AUX",
            AudioSourceTarget::Bluetooth,
            SourceMutationReply::Success,
            "BT",
            true,
        ),
        AudioSourceWriteResult::Applied(MediaSource::Bluetooth)
    );
}

#[test]
fn source_already_at_target_sends_no_mutation() {
    assert_eq!(
        run_source_prewrite_fixture(
            "AUX",
            source_list_with_target_and_unknown(),
            Some(Some(9)),
            AudioSourceTarget::AuxIn,
        ),
        AudioSourceWriteResult::AlreadyAtTarget(MediaSource::AuxIn)
    );
}

#[test]
fn source_target_absent_from_dynamic_list_sends_no_mutation() {
    assert_eq!(
        run_source_prewrite_fixture(
            "BT",
            r#"{"error_code":0,"audiosource_info":{"support_sources":[{"source":"BT","type":1},{"source":"PRIVATE","type":9}]}}"#,
            None,
            AudioSourceTarget::AuxIn,
        ),
        AudioSourceWriteResult::RejectedBeforeSend(JblError::UnsupportedMediaSource)
    );
}

#[test]
fn source_list_rejects_noninteger_type_and_duplicate_known_target() {
    for source_list in [
        r#"{"error_code":0,"audiosource_info":{"support_sources":[{"source":"AUX","type":"2"}]}}"#,
        r#"{"error_code":0,"audiosource_info":{"support_sources":[{"source":"AUX","type":2},{"source":"AUX","type":3}]}}"#,
    ] {
        assert_eq!(
            run_source_prewrite_fixture("BT", source_list, None, AudioSourceTarget::AuxIn),
            AudioSourceWriteResult::RejectedBeforeSend(JblError::MediaSourceInvalid)
        );
    }
}

#[test]
fn source_list_ignores_unknown_entries_without_promoting_them() {
    assert_eq!(
        run_source_prewrite_fixture(
            "BT",
            r#"{"error_code":0,"audiosource_info":{"support_sources":[{"source":"PRIVATE","type":9}]}}"#,
            None,
            AudioSourceTarget::AuxIn,
        ),
        AudioSourceWriteResult::RejectedBeforeSend(JblError::UnsupportedMediaSource)
    );
    assert_eq!(
        run_source_mutation_fixture(
            "BT",
            AudioSourceTarget::AuxIn,
            SourceMutationReply::Success,
            "AUX",
            true,
        ),
        AudioSourceWriteResult::Applied(MediaSource::AuxIn)
    );
}

#[test]
fn source_rejects_missing_or_unsafe_volume_before_write() {
    assert_eq!(
        run_source_prewrite_fixture(
            "BT",
            source_list_with_target_and_unknown(),
            Some(None),
            AudioSourceTarget::AuxIn,
        ),
        AudioSourceWriteResult::RejectedBeforeSend(JblError::MediaVolumeMissing)
    );
    assert_eq!(
        run_source_prewrite_fixture(
            "BT",
            source_list_with_target_and_unknown(),
            Some(Some(10)),
            AudioSourceTarget::AuxIn,
        ),
        AudioSourceWriteResult::RejectedBeforeSend(JblError::VolumeSafetyLimitExceeded)
    );
}

#[test]
fn source_nonzero_basic_response_is_device_rejection() {
    assert_eq!(
        run_source_mutation_fixture(
            "BT",
            AudioSourceTarget::AuxIn,
            SourceMutationReply::Rejected,
            "BT",
            true,
        ),
        AudioSourceWriteResult::RejectedByDevice(MediaSource::Bluetooth)
    );
}

#[test]
fn source_disconnect_after_exact_body_is_one_write_and_only_weak_readback_evidence() {
    assert_eq!(
        run_source_mutation_fixture(
            "BT",
            AudioSourceTarget::AuxIn,
            SourceMutationReply::Lost,
            "AUX",
            true,
        ),
        AudioSourceWriteResult::TargetObservedAfterUnknownWrite(MediaSource::AuxIn)
    );
}

#[test]
fn source_http_success_with_conflicting_readback_fails_postcondition() {
    assert_eq!(
        run_source_mutation_fixture(
            "BT",
            AudioSourceTarget::AuxIn,
            SourceMutationReply::Success,
            "BT",
            true,
        ),
        AudioSourceWriteResult::PostconditionFailed(MediaSource::Bluetooth)
    );
}

#[test]
fn source_post_write_pinned_identity_failure_is_unknown() {
    assert_eq!(
        run_source_mutation_fixture(
            "BT",
            AudioSourceTarget::AuxIn,
            SourceMutationReply::Success,
            "AUX",
            false,
        ),
        AudioSourceWriteResult::OutcomeUnknown(JblError::DeviceInfoMissing)
    );
}

#[derive(Clone, Copy)]
enum EqMutationReply {
    Success,
    Rejected,
    Lost,
}

fn eq_feature_response(support: &str, band: &str, preset_support: &str) -> String {
    serde_json::json!({
        "error_code": 0,
        "feature_support": {
            "user_eq": {
                "support": support,
                "band": band,
                "preset_support": preset_support
            }
        }
    })
    .to_string()
}

fn valid_eq_feature_response() -> String {
    eq_feature_response("true", "7", "true")
}

fn eq_entry(id: &str, name: &str, fs: [i64; 7], gain: [i64; 7]) -> serde_json::Value {
    serde_json::json!({
        "band": 7,
        "eq_id": id,
        "eq_name": name,
        "eq_payload": {"fs": fs, "gain": gain}
    })
}

fn eq_catalog_value(active: &str) -> serde_json::Value {
    serde_json::json!({
        "error_code": 0,
        "active_eq_id": active,
        "eq_list": [
            eq_entry("1", "JBL SIGNATURE", [101,102,103,104,105,106,107], [1,2,3,4,5,6,7]),
            eq_entry("2", "VOCAL", [201,202,203,204,205,206,207], [-1,-2,-3,-4,-5,-6,-7]),
            eq_entry("3", "ENERGETIC", [301,302,303,304,305,306,307], [3,4,5,6,7,8,9]),
            eq_entry("4", "CHILL", [401,402,403,404,405,406,407], [4,5,6,7,8,9,10]),
            eq_entry("0", "CUSTOMIZE", [501,502,503,504,505,506,507], [0,0,0,0,0,0,0])
        ]
    })
}

fn eq_catalog_response(active: &str) -> String {
    eq_catalog_value(active).to_string()
}

fn expected_eq_mutation_body(target: EqPresetTarget) -> &'static [u8] {
    match target {
        EqPresetTarget::Signature => concat!(
            "command=setActiveEQ&payload={\"active_eq_id\":\"1\",\"band\":7,",
            "\"eq_payload\":{\"gain\":[1,2,3,4,5,6,7],",
            "\"fs\":[101,102,103,104,105,106,107]}}"
        )
        .as_bytes(),
        EqPresetTarget::Vocal => concat!(
            "command=setActiveEQ&payload={\"active_eq_id\":\"2\",\"band\":7,",
            "\"eq_payload\":{\"gain\":[-1,-2,-3,-4,-5,-6,-7],",
            "\"fs\":[201,202,203,204,205,206,207]}}"
        )
        .as_bytes(),
        EqPresetTarget::Energetic => concat!(
            "command=setActiveEQ&payload={\"active_eq_id\":\"3\",\"band\":7,",
            "\"eq_payload\":{\"gain\":[3,4,5,6,7,8,9],",
            "\"fs\":[301,302,303,304,305,306,307]}}"
        )
        .as_bytes(),
        EqPresetTarget::Chill => concat!(
            "command=setActiveEQ&payload={\"active_eq_id\":\"4\",\"band\":7,",
            "\"eq_payload\":{\"gain\":[4,5,6,7,8,9,10],",
            "\"fs\":[401,402,403,404,405,406,407]}}"
        )
        .as_bytes(),
    }
}

fn assert_eq_mutation_request(request: &CapturedHttpRequest, target: EqPresetTarget) {
    let expected = expected_eq_mutation_body(target);
    assert_eq!(request.header("content-type"), None);
    assert_eq!(request.header("accept"), Some("application/json"));
    assert_eq!(request.header("accept-encoding"), Some("identity"));
    assert_eq!(request.header("transfer-encoding"), None);
    assert_eq!(request.body, expected);
    assert_eq!(
        request.header("content-length"),
        Some(expected.len().to_string().as_str())
    );
    assert!(!request.body.contains(&b'%'));
    assert_eq!(request.request_line, "POST /httpapi.asp? HTTP/1.1");
}

fn run_eq_mutation_fixture(
    before_active: &'static str,
    target: EqPresetTarget,
    reply: EqMutationReply,
    after_active: &'static str,
    post_identity_valid: bool,
) -> EqPresetWriteResult {
    let files = PrivateIdentityFiles::create();
    let before_catalog = eq_catalog_response(before_active);
    let after_catalog = eq_catalog_response(after_active);
    let feature = valid_eq_feature_response();
    let tls_server = spawn_tls_server_connections(6, move |index, stream, request| match index {
        0 | 5 => {
            assert!(request.request_line.contains("command=getDeviceInfo"));
            let body: &[u8] = if index == 5 && !post_identity_valid {
                br#"{"error_code":0}"#
            } else {
                br#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#
            };
            write_response(stream, "200 OK", "Content-Type: application/json\r\n", body);
        }
        1 => {
            assert!(request.request_line.contains("command=getFeatureSupport"));
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                feature.as_bytes(),
            );
        }
        2 | 4 => {
            assert!(request.request_line.contains("command=getEQList"));
            let body = if index == 2 {
                before_catalog.as_bytes()
            } else {
                after_catalog.as_bytes()
            };
            write_response(stream, "200 OK", "Content-Type: application/json\r\n", body);
        }
        3 => {
            assert_eq_mutation_request(&request, target);
            match reply {
                EqMutationReply::Success => write_response(
                    stream,
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    br#"{"error_code":0}"#,
                ),
                EqMutationReply::Rejected => write_response(
                    stream,
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    br#"{"error_code":9}"#,
                ),
                EqMutationReply::Lost => {}
            }
        }
        _ => unreachable!(),
    });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(2, move |index, stream, request| {
        if index == 0 {
            assert_model_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                model_body.as_bytes(),
            );
        } else {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(9)).as_bytes(),
            );
        }
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();
    let result = client.set_eq_preset(&mut lock, SUPPORTED_JBL_MODEL, target);
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
    result
}

fn run_eq_prewrite_fixture(
    feature: String,
    catalog: Option<String>,
    volume_read: Option<Option<u8>>,
    target: EqPresetTarget,
) -> EqPresetWriteResult {
    let files = PrivateIdentityFiles::create();
    let tls_server = spawn_tls_server_connections(
        2 + usize::from(catalog.is_some()),
        move |index, stream, request| {
            let body = match index {
                0 => {
                    assert!(request.request_line.contains("command=getDeviceInfo"));
                    r#"{"error_code":"0","device_info":{"firmware":"1","one_os_ver":"3"}}"#
                        .to_string()
                }
                1 => {
                    assert!(request.request_line.contains("command=getFeatureSupport"));
                    feature.clone()
                }
                2 => {
                    assert!(request.request_line.contains("command=getEQList"));
                    catalog.clone().expect("fixture catalog should exist")
                }
                _ => unreachable!(),
            };
            write_response(
                stream,
                "200 OK",
                "Content-Type: application/json\r\n",
                body.as_bytes(),
            );
        },
    );
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(
        1 + usize::from(volume_read.is_some()),
        move |index, stream, request| {
            if index == 0 {
                assert_model_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    model_body.as_bytes(),
                );
            } else {
                assert_info_request(&request);
                write_response(
                    stream,
                    "200 OK",
                    "Content-Type: text/xml\r\n",
                    upnp_info_body(volume_read.flatten()).as_bytes(),
                );
            }
        },
    );
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let mut lock = DirectControlLock::for_protocol_fixture();
    let result = client.set_eq_preset(&mut lock, SUPPORTED_JBL_MODEL, target);
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();
    result
}

#[test]
fn eq_vocal_and_restore_signature_use_exact_complete_fixture_payloads() {
    assert_eq!(
        run_eq_mutation_fixture(
            "1",
            EqPresetTarget::Vocal,
            EqMutationReply::Success,
            "2",
            true
        ),
        EqPresetWriteResult::Applied(EqPresetTarget::Vocal)
    );
    assert_eq!(
        run_eq_mutation_fixture(
            "2",
            EqPresetTarget::Signature,
            EqMutationReply::Success,
            "1",
            true
        ),
        EqPresetWriteResult::Applied(EqPresetTarget::Signature)
    );
}

#[test]
fn eq_already_at_target_sends_no_mutation() {
    assert_eq!(
        run_eq_prewrite_fixture(
            valid_eq_feature_response(),
            Some(eq_catalog_response("2")),
            Some(Some(9)),
            EqPresetTarget::Vocal,
        ),
        EqPresetWriteResult::AlreadyAtTarget(EqPresetTarget::Vocal)
    );
}

#[test]
fn eq_feature_support_band_and_preset_false_send_no_mutation() {
    for feature in [
        eq_feature_response("false", "7", "true"),
        eq_feature_response("true", "3", "true"),
        eq_feature_response("true", "7", "false"),
    ] {
        assert_eq!(
            run_eq_prewrite_fixture(feature, None, None, EqPresetTarget::Vocal),
            EqPresetWriteResult::RejectedBeforeSend(JblError::EqPresetInvalid)
        );
    }
}

#[test]
fn eq_missing_or_unavailable_target_and_duplicate_name_or_id_send_no_mutation() {
    let missing_catalog = serde_json::json!({"error_code":0}).to_string();
    let mut unavailable = eq_catalog_value("1");
    unavailable["eq_list"][1]["eq_name"] = serde_json::json!("PRIVATE");
    let mut duplicate_target = eq_catalog_value("1");
    duplicate_target["eq_list"][2]["eq_name"] = serde_json::json!("VOCAL");
    let mut duplicate_id = eq_catalog_value("1");
    duplicate_id["eq_list"][2]["eq_id"] = serde_json::json!("2");
    for catalog in [
        missing_catalog,
        unavailable.to_string(),
        duplicate_target.to_string(),
        duplicate_id.to_string(),
    ] {
        assert_eq!(
            run_eq_prewrite_fixture(
                valid_eq_feature_response(),
                Some(catalog),
                None,
                EqPresetTarget::Vocal,
            ),
            EqPresetWriteResult::RejectedBeforeSend(JblError::EqPresetInvalid)
        );
    }
}

#[test]
fn eq_bad_band_array_length_type_and_missing_active_send_no_mutation() {
    let mut bad_band = eq_catalog_value("1");
    bad_band["eq_list"][0]["band"] = serde_json::json!(6);
    let mut bad_length = eq_catalog_value("1");
    bad_length["eq_list"][1]["eq_payload"]["gain"] = serde_json::json!([1, 2]);
    let mut bad_type = eq_catalog_value("1");
    bad_type["eq_list"][1]["eq_payload"]["fs"][0] = serde_json::json!("bad");
    let mut missing_active = eq_catalog_value("1");
    missing_active
        .as_object_mut()
        .unwrap()
        .remove("active_eq_id");
    for catalog in [bad_band, bad_length, bad_type, missing_active] {
        assert_eq!(
            run_eq_prewrite_fixture(
                valid_eq_feature_response(),
                Some(catalog.to_string()),
                None,
                EqPresetTarget::Vocal,
            ),
            EqPresetWriteResult::RejectedBeforeSend(JblError::EqPresetInvalid)
        );
    }
}

#[test]
fn eq_custom_active_and_unsafe_volume_send_no_mutation() {
    assert_eq!(
        run_eq_prewrite_fixture(
            valid_eq_feature_response(),
            Some(eq_catalog_response("0")),
            Some(Some(9)),
            EqPresetTarget::Vocal,
        ),
        EqPresetWriteResult::RejectedBeforeSend(JblError::EqPresetInvalid)
    );
    assert_eq!(
        run_eq_prewrite_fixture(
            valid_eq_feature_response(),
            Some(eq_catalog_response("1")),
            Some(None),
            EqPresetTarget::Vocal,
        ),
        EqPresetWriteResult::RejectedBeforeSend(JblError::MediaVolumeMissing)
    );
    assert_eq!(
        run_eq_prewrite_fixture(
            valid_eq_feature_response(),
            Some(eq_catalog_response("1")),
            Some(Some(10)),
            EqPresetTarget::Vocal,
        ),
        EqPresetWriteResult::RejectedBeforeSend(JblError::VolumeSafetyLimitExceeded)
    );
}

#[test]
fn eq_nonzero_basic_response_is_device_rejection() {
    assert_eq!(
        run_eq_mutation_fixture(
            "1",
            EqPresetTarget::Vocal,
            EqMutationReply::Rejected,
            "1",
            true
        ),
        EqPresetWriteResult::RejectedByDevice(EqPresetTarget::Vocal)
    );
}

#[test]
fn eq_disconnect_after_complete_body_is_one_write_and_only_weak_readback_evidence() {
    assert_eq!(
        run_eq_mutation_fixture("1", EqPresetTarget::Vocal, EqMutationReply::Lost, "2", true),
        EqPresetWriteResult::TargetObservedAfterUnknownWrite(EqPresetTarget::Vocal)
    );
}

#[test]
fn eq_http_success_with_conflicting_readback_fails_postcondition() {
    assert_eq!(
        run_eq_mutation_fixture(
            "1",
            EqPresetTarget::Vocal,
            EqMutationReply::Success,
            "1",
            true
        ),
        EqPresetWriteResult::PostconditionFailed(Some(EqPresetTarget::Signature))
    );
}

#[test]
fn eq_post_write_pinned_identity_failure_is_unknown() {
    assert_eq!(
        run_eq_mutation_fixture(
            "1",
            EqPresetTarget::Vocal,
            EqMutationReply::Success,
            "2",
            false
        ),
        EqPresetWriteResult::OutcomeUnknown(JblError::DeviceInfoMissing)
    );
}

#[test]
fn direct_read_is_exactly_ten_reads_and_projects_no_raw_device_values() {
    let files = PrivateIdentityFiles::create();
    let raw_marker = "private-sentinel";
    let mut feature: serde_json::Value =
        serde_json::from_str(&valid_eq_feature_response()).unwrap();
    feature["feature_support"][raw_marker] = serde_json::json!({"support":"true"});
    let mut catalog = eq_catalog_value(raw_marker);
    catalog["active_eq_id"] = serde_json::json!(raw_marker);
    catalog["eq_list"][0]["eq_id"] = serde_json::json!(raw_marker);
    catalog["eq_list"][0]["eq_payload"]["gain"][0] = serde_json::json!(987654);
    let responses = [
        r#"{"error_code":0,"device_info":{"firmware":"1","one_os_ver":"3"}}"#
            .to_string(),
        feature.to_string(),
        catalog.to_string(),
        r#"{"error_code":0,"eq_setting":{"eq_id":"private-current-id","eq_name":"private-current-name","eq_status":"on","eq_payload":{"fs":[1,2,3],"gain":[4,5,6],"q":[7,8,9],"type":[10,11,12]}}}"#.to_string(),
        r#"{"error_code":0,"audiosource_info":{"active_source":"BT","support_sources":[{"source":"BT","type":1},{"source":"AUX","type":2},{"source":"PRIVATE_SOURCE","type":9}]}}"#.to_string(),
        r#"{"error_code":0,"status":"off"}"#.to_string(),
        r#"{"error_code":0,"audio_sync":"0"}"#.to_string(),
        r#"{"error_code":0,"media_source":"BT","media_status":"stopped"}"#.to_string(),
    ];
    let commands = [
        "getDeviceInfo",
        "getFeatureSupport",
        "getEQList",
        "getEQ",
        "getDeviceAudioSourceList",
        "getPersonalListeningMode",
        "getAudioSync",
        "getMediaSourceStatus",
    ];
    let tls_server = spawn_tls_server_connections(8, move |index, stream, request| {
        assert_eq!(
            request.request_line,
            format!("GET /httpapi.asp?command={} HTTP/1.1", commands[index])
        );
        write_response(
            stream,
            "200 OK",
            "Content-Type: application/json\r\n",
            responses[index].as_bytes(),
        );
    });
    let model_body = upnp_model_body(SUPPORTED_JBL_MODEL);
    let upnp_server = spawn_http_server_connections(2, move |index, stream, request| {
        if index == 0 {
            assert_model_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                model_body.as_bytes(),
            );
        } else {
            assert_info_request(&request);
            write_response(
                stream,
                "200 OK",
                "Content-Type: text/xml\r\n",
                upnp_info_body(Some(9)).as_bytes(),
            );
        }
    });
    let client = test_client(
        &files,
        &identity_material().fingerprint,
        Duration::from_secs(1),
        tls_server.port,
        upnp_server.port,
    );
    let observed = client
        .direct_read(SUPPORTED_JBL_MODEL)
        .expect("direct read fixture should pass");
    tls_server.finish_and_assert_no_extra_connection();
    upnp_server.finish_and_assert_no_extra_connection();

    assert_eq!(observed.media.source, MediaSource::Bluetooth);
    assert_eq!(observed.media.playback.volume, Some(9));
    assert_eq!(observed.inspection.eq.preset_count, 5);
    assert_eq!(observed.inspection.feature_support.unknown_key_count, 1);
    assert_eq!(
        observed.source_targets,
        vec![AudioSourceTarget::Bluetooth, AudioSourceTarget::AuxIn]
    );
    assert_eq!(observed.active_eq, Some(EqPresetTarget::Signature));

    let safe_debug = format!(
        "{:?}",
        (
            &observed.media,
            &observed.inspection,
            &observed.source_targets,
            observed.active_eq
        )
    );
    let safe_json = serde_json::json!({
        "media": observed.media,
        "inspection": observed.inspection,
        "source_targets": observed.source_targets,
        "active_eq": observed.active_eq,
    })
    .to_string();
    for marker in [
        raw_marker,
        "private-current-id",
        "private-current-name",
        "PRIVATE_SOURCE",
        "987654",
    ] {
        assert!(!safe_debug.contains(marker));
        assert!(!safe_json.contains(marker));
    }
}

#[test]
fn raw_http_response_parser_rejects_ambiguous_and_unbounded_shapes() {
    let invalid = [
        b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nBroken\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".as_slice(),
    ];
    for response in invalid {
        assert_eq!(
            super::parse_source_http_response(response),
            Err(JblError::InvalidHttpResponse)
        );
    }
    let oversized_header = format!(
        "HTTP/1.1 200 OK\r\nX-Fill: {}\r\nContent-Length: 0\r\n\r\n",
        "x".repeat(super::MAX_SOURCE_RESPONSE_HEADER_BYTES)
    );
    assert_eq!(
        super::parse_source_http_response(oversized_header.as_bytes()),
        Err(JblError::ResponseTooLarge)
    );
    let oversized_body = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        super::MAX_RESPONSE_BYTES + 1
    );
    assert_eq!(
        super::parse_source_http_response(oversized_body.as_bytes()),
        Err(JblError::InvalidHttpResponse)
    );
}

#[test]
fn raw_deadline_stream_rejects_drip_feed_at_one_absolute_deadline() {
    let listener = TcpListener::bind((LOOPBACK, 0)).expect("deadline fixture should bind");
    let address = listener.local_addr().expect("deadline fixture address");
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("deadline fixture accept");
        for byte in b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}" {
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
    });
    let socket = TcpStream::connect(address).expect("deadline fixture connect");
    let started = Instant::now();
    let mut stream = super::DeadlineTcpStream {
        inner: socket,
        deadline: started + Duration::from_millis(120),
    };
    let mut byte = [0_u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(1) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                break;
            }
            result => panic!("unexpected deadline fixture result: {result:?}"),
        }
    }
    assert!(started.elapsed() < Duration::from_millis(300));
    worker.join().expect("deadline fixture worker");
}
