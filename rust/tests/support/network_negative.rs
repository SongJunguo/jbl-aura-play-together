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

use super::{ClientPorts, Command, JblLanClient, MAX_RESPONSE_BYTES};
use crate::control::{PlayTogetherCommand, PlayTogetherWriteOutcome, PlayTogetherWriteResult};
use crate::error::JblError;
use crate::model::{DeviceIdentity, SUPPORTED_JBL_MODEL};

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
        client.get_json(Command::DeviceInfo).unwrap_err(),
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
        client.get_json(Command::DeviceInfo).unwrap_err(),
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
        client.get_json(Command::DeviceInfo).unwrap_err(),
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
        client.get_json(Command::DeviceInfo).unwrap_err(),
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
        client.get_json(Command::DeviceInfo).unwrap_err(),
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
