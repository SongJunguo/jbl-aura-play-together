//! Bounded, proxy-free client for the loopback-only Rust service.
//!
//! This client intentionally uses `TcpStream` directly.  It therefore cannot
//! inherit HTTP proxy settings, follow redirects, resolve an attacker supplied
//! hostname, or leave loopback.  Mutations always fetch a fresh CSRF cookie and
//! revision first; a 409 is returned to the caller and is never retried.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::web::DEFAULT_WEB_PORT;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const READY_TIMEOUT: Duration = Duration::from_secs(2);
const STATUS_TIMEOUT: Duration = Duration::from_secs(120);
const ACTION_TIMEOUT_SECONDS: u64 = 600;
const ACTION_TIMEOUT: Duration = Duration::from_secs(ACTION_TIMEOUT_SECONDS);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADER_COUNT: usize = 64;
const CSRF_COOKIE: &str = "jbl_aura_csrf";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalClientError {
    Unavailable,
    TimedOut,
    ResponseTooLarge,
    InvalidResponse,
    ServiceRejected,
    RevisionConflict,
}

impl LocalClientError {
    pub const fn service_unavailable(self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

impl fmt::Display for LocalClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "the local Play Together service is unavailable",
            Self::TimedOut => "the local Play Together service timed out",
            Self::ResponseTooLarge => "the local Play Together response exceeded its limit",
            Self::InvalidResponse => "the local Play Together service returned an invalid response",
            Self::ServiceRejected => "the local Play Together service rejected the request",
            Self::RevisionConflict => {
                "the local Play Together state changed; run the command again"
            }
        })
    }
}

impl std::error::Error for LocalClientError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalBackend {
    LegacyV04WholePair,
    NativePair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalPairConfiguration {
    Ready,
    NotReady,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalPairMemberName {
    #[serde(rename = "JBL Authentics 300")]
    JblAuthentics300,
    #[serde(rename = "Aura Studio 5")]
    AuraStudio5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalPairMemberVerification {
    Verified,
    NotVerified,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalPairMemberChannel {
    FrontLeft,
    FrontRight,
    Left,
    Right,
    Mono,
    Stereo,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPairMember {
    pub name: LocalPairMemberName,
    pub verification: LocalPairMemberVerification,
    pub channels: Vec<LocalPairMemberChannel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalManagedState {
    Unknown,
    Offline,
    Ready,
    Linking,
    Linked,
    Unlinking,
    Recovering,
    Degraded,
    ShuttingDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalLifecycle {
    Offline,
    Initializing,
    Connecting,
    Ready,
    Linking,
    Linked,
    Unlinking,
    Degraded,
    Recovering,
    ShuttingDown,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalHealthLevel {
    Healthy,
    Transitioning,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAuraTransport {
    Le,
    BrEdr,
    Unresolved,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAuraAcquisitionRoute {
    StableDirect,
    A2dpWakeThenStable,
    FreshLe,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalHealth {
    pub lifecycle: LocalLifecycle,
    pub level: LocalHealthLevel,
    pub reported_error: bool,
    pub aura_transport: LocalAuraTransport,
    pub aura_acquisition_route: LocalAuraAcquisitionRoute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalStatus {
    pub backend: LocalBackend,
    pub pair_configuration: LocalPairConfiguration,
    pub members: [LocalPairMember; 2],
    pub managed_state: LocalManagedState,
    pub backend_health: Option<LocalHealth>,
    pub unresolved_action: bool,
    pub consecutive_failures: u8,
    pub revision: u64,
    pub last_action: Option<LocalLastAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalActionName {
    Start,
    Stop,
    Shutdown,
    RecoverStop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalActionOutcome {
    Accepted,
    AcceptedUnconfirmed,
    Idempotent,
    RejectedBeforeSend,
    OutcomeUnknown,
    PostconditionFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalActionEvidence {
    LocalSessionState,
    LifecycleAcknowledgement,
    BroadcastAcknowledgementOnly,
    BroadcastBusinessNotification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalFailure {
    PairConfigurationUnavailable,
    ExpectedPairNotConfigured,
    BackendRejectedBeforeSend,
    AuraInvalidConfiguration,
    AuraRuntimeUnavailable,
    AuraAdapterUnavailable,
    AuraDiscoveryUnavailable,
    AuraVerifiedAdvertisementNotFound,
    AuraDeviceConnectionFailed,
    WakeProfileConnectFailed,
    WakeFddfTimedOut,
    WakeFddfInvalid,
    WakeFddfUnavailable,
    WakeProfileReleaseFailed,
    AuraGattProfileInvalid,
    AuraNotificationSetupFailed,
    AuraTransportNotReady,
    AuraNotificationQueueInvalid,
    AuraDisconnectFailed,
    AuraWriteUnknown,
    AuraAckTimeout,
    AuraAckChannelClosed,
    AuraUnexpectedAck,
    JblEnterOutcomeUnknown,
    JblExitOutcomeUnknown,
    JblBroadcastResultTimedOut,
    JblBroadcastResultUnavailable,
    JblBroadcastResultRejected,
    AuraStartOutcomeUnknown,
    BackendOutcomeUnknown,
    UnexpectedBackendLifecycle,
    MembershipPostconditionFailed,
    UnresolvedPriorAction,
    RecoveryNotAllowed,
    JournalUnavailable,
    JournalCommitFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Evidence is present only for a newly accepted backend action. ACK-only is
/// intentionally paired with `AcceptedUnconfirmed`, never `Accepted`.
pub struct LocalActionResult {
    pub action: LocalActionName,
    pub outcome: LocalActionOutcome,
    pub managed_state: LocalManagedState,
    pub evidence: Option<LocalActionEvidence>,
    pub failure: Option<LocalFailure>,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalLastAction {
    pub action: LocalActionName,
    pub outcome: LocalActionOutcome,
    pub evidence: Option<LocalActionEvidence>,
    pub failure: Option<LocalFailure>,
    pub revision: u64,
    pub age_ms: u64,
}

impl LocalActionResult {
    pub const fn succeeded(self) -> bool {
        matches!(
            self.outcome,
            LocalActionOutcome::Accepted
                | LocalActionOutcome::AcceptedUnconfirmed
                | LocalActionOutcome::Idempotent
        )
    }
}

/// Client fixed to IPv4 loopback.  The public constructor has no address
/// argument, so product code cannot redirect control requests elsewhere.
#[derive(Clone, Copy)]
pub struct LocalServiceClient {
    address: SocketAddr,
}

impl Default for LocalServiceClient {
    fn default() -> Self {
        Self {
            address: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, DEFAULT_WEB_PORT)),
        }
    }
}

impl LocalServiceClient {
    #[cfg(test)]
    fn for_test(address: SocketAddr) -> Self {
        assert!(address.ip().is_loopback());
        Self { address }
    }

    /// Lightweight listener readiness check.  It never calls the controller
    /// actor and therefore never waits for a device read.
    pub fn ready(&self) -> Result<(), LocalClientError> {
        let response = self.request("GET", "/healthz", &[], &[], READY_TIMEOUT)?;
        if response.status == 200 && response.body == br#"{"status":"ready"}"# {
            Ok(())
        } else {
            Err(LocalClientError::InvalidResponse)
        }
    }

    pub fn status(&self) -> Result<LocalStatus, LocalClientError> {
        let response = self.request("GET", "/api/status", &[], &[], STATUS_TIMEOUT)?;
        if response.status != 200 {
            return Err(LocalClientError::ServiceRejected);
        }
        let status: LocalStatus = serde_json::from_slice(&response.body)
            .map_err(|_| LocalClientError::InvalidResponse)?;
        if !valid_status_projection(&status) {
            return Err(LocalClientError::InvalidResponse);
        }
        let expected_etag = format!("\"{}\"", status.revision);
        if response.headers.get("etag") != Some(&expected_etag) {
            return Err(LocalClientError::InvalidResponse);
        }
        Ok(status)
    }

    pub fn start(&self) -> Result<LocalActionResult, LocalClientError> {
        self.mutate("/api/start", LocalActionName::Start)
    }

    pub fn stop(&self) -> Result<LocalActionResult, LocalClientError> {
        self.mutate("/api/stop", LocalActionName::Stop)
    }

    pub fn recover_stop(&self) -> Result<LocalActionResult, LocalClientError> {
        self.mutate_with_body(
            "/internal/recover-stop",
            LocalActionName::RecoverStop,
            br#"{"confirm":"recover-stop"}"#,
        )
    }

    fn mutate(
        &self,
        path: &'static str,
        expected_action: LocalActionName,
    ) -> Result<LocalActionResult, LocalClientError> {
        self.mutate_with_body(path, expected_action, b"{}")
    }

    fn mutate_with_body(
        &self,
        path: &'static str,
        expected_action: LocalActionName,
        body: &'static [u8],
    ) -> Result<LocalActionResult, LocalClientError> {
        // Fetch both values immediately before the one POST.  In particular,
        // a conflict never causes this sequence or the POST to repeat.
        let csrf = self.csrf_cookie()?;
        let status = self.status()?;
        let etag = format!("\"{}\"", status.revision);
        let cookie = format!("{CSRF_COOKIE}={}", csrf.as_str());
        let mut headers = [
            ("Origin", self.origin()),
            ("Content-Type", "application/json".to_string()),
            ("If-Match", etag),
            ("X-CSRF-Token", csrf.to_string()),
            ("Cookie", cookie),
        ];
        let response_result = self.request("POST", path, &headers, body, ACTION_TIMEOUT);
        for (_, value) in &mut headers {
            value.zeroize();
        }
        let response = response_result?;
        if response.status == 409 {
            return Err(LocalClientError::RevisionConflict);
        }
        if response.status != 200 {
            return Err(LocalClientError::ServiceRejected);
        }
        let result: LocalActionResult = serde_json::from_slice(&response.body)
            .map_err(|_| LocalClientError::InvalidResponse)?;
        if result.action != expected_action
            || result.revision <= status.revision
            || !valid_action_projection(&result)
        {
            return Err(LocalClientError::InvalidResponse);
        }
        let response_etag = format!("\"{}\"", result.revision);
        if response.headers.get("etag") != Some(&response_etag) {
            return Err(LocalClientError::InvalidResponse);
        }
        Ok(result)
    }

    fn csrf_cookie(&self) -> Result<Zeroizing<String>, LocalClientError> {
        let response = self.request("GET", "/", &[], &[], READY_TIMEOUT)?;
        if response.status != 200 {
            return Err(LocalClientError::ServiceRejected);
        }
        let cookie = response
            .headers
            .get("set-cookie")
            .ok_or(LocalClientError::InvalidResponse)?;
        let first = cookie
            .split(';')
            .next()
            .ok_or(LocalClientError::InvalidResponse)?;
        let (name, token) = first
            .split_once('=')
            .ok_or(LocalClientError::InvalidResponse)?;
        if name != CSRF_COOKIE
            || token.len() != 64
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(LocalClientError::InvalidResponse);
        }
        Ok(Zeroizing::new(token.to_string()))
    }

    fn request(
        &self,
        method: &'static str,
        path: &'static str,
        headers: &[(&str, String)],
        body: &[u8],
        timeout: Duration,
    ) -> Result<LocalHttpResponse, LocalClientError> {
        let deadline = Instant::now() + timeout;
        let connect_timeout = remaining(deadline)?.min(CONNECT_TIMEOUT);
        let mut stream = TcpStream::connect_timeout(&self.address, connect_timeout)
            .map_err(map_connect_error)?;
        let host = self.authority();
        let mut request = Zeroizing::new(format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nAccept-Encoding: identity\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        )
        .into_bytes());
        for (name, value) in headers {
            let name = *name;
            if !valid_header_name(name) || !valid_header_value(value) {
                return Err(LocalClientError::InvalidResponse);
            }
            request.extend_from_slice(name.as_bytes());
            request.extend_from_slice(b": ");
            request.extend_from_slice(value.as_bytes());
            request.extend_from_slice(b"\r\n");
        }
        request.extend_from_slice(b"\r\n");
        request.extend_from_slice(body);
        write_all_before(&mut stream, &request, deadline)?;
        let payload = read_to_end_before(&mut stream, deadline)?;
        parse_response(&payload)
    }

    fn authority(&self) -> String {
        format!("127.0.0.1:{}", self.address.port())
    }

    fn origin(&self) -> String {
        format!("http://{}", self.authority())
    }
}

fn valid_status_projection(status: &LocalStatus) -> bool {
    if status.members[0].name != LocalPairMemberName::JblAuthentics300
        || status.members[1].name != LocalPairMemberName::AuraStudio5
        || status
            .members
            .iter()
            .any(|member| !valid_member_projection(member))
        || status.last_action.is_some_and(|action| {
            action.revision > status.revision
                || !valid_outcome_projection(action.outcome, action.evidence, action.failure)
        })
        || status
            .backend_health
            .is_some_and(|health| !valid_health_projection(health))
    {
        return false;
    }
    match status.pair_configuration {
        LocalPairConfiguration::Ready => status
            .members
            .iter()
            .all(|member| member.verification == LocalPairMemberVerification::Verified),
        LocalPairConfiguration::NotReady => status
            .members
            .iter()
            .all(|member| member.verification != LocalPairMemberVerification::Unavailable),
        LocalPairConfiguration::Unavailable => status
            .members
            .iter()
            .all(|member| member.verification == LocalPairMemberVerification::Unavailable),
    }
}

fn valid_action_projection(result: &LocalActionResult) -> bool {
    if !valid_outcome_projection(result.outcome, result.evidence, result.failure) {
        return false;
    }
    if result.outcome == LocalActionOutcome::Idempotent
        && result.action == LocalActionName::RecoverStop
    {
        return false;
    }
    let expected_success_state = match result.action {
        LocalActionName::Start => LocalManagedState::Linked,
        LocalActionName::Stop | LocalActionName::RecoverStop => LocalManagedState::Ready,
        LocalActionName::Shutdown => LocalManagedState::Offline,
    };
    match result.outcome {
        LocalActionOutcome::Accepted
        | LocalActionOutcome::AcceptedUnconfirmed
        | LocalActionOutcome::Idempotent => result.managed_state == expected_success_state,
        LocalActionOutcome::OutcomeUnknown | LocalActionOutcome::PostconditionFailed => {
            result.managed_state == LocalManagedState::Unknown
        }
        LocalActionOutcome::RejectedBeforeSend => !matches!(
            result.managed_state,
            LocalManagedState::Linking
                | LocalManagedState::Unlinking
                | LocalManagedState::Recovering
                | LocalManagedState::ShuttingDown
        ),
    }
}

fn valid_outcome_projection(
    outcome: LocalActionOutcome,
    evidence: Option<LocalActionEvidence>,
    failure: Option<LocalFailure>,
) -> bool {
    match outcome {
        LocalActionOutcome::Accepted => {
            failure.is_none()
                && evidence
                    .is_some_and(|value| value != LocalActionEvidence::BroadcastAcknowledgementOnly)
        }
        LocalActionOutcome::AcceptedUnconfirmed => {
            evidence == Some(LocalActionEvidence::BroadcastAcknowledgementOnly) && failure.is_none()
        }
        LocalActionOutcome::Idempotent => evidence.is_none() && failure.is_none(),
        LocalActionOutcome::RejectedBeforeSend | LocalActionOutcome::OutcomeUnknown => {
            evidence.is_none() && failure.is_some()
        }
        LocalActionOutcome::PostconditionFailed => {
            evidence.is_none() && failure == Some(LocalFailure::MembershipPostconditionFailed)
        }
    }
}

fn valid_health_projection(health: LocalHealth) -> bool {
    match health.aura_acquisition_route {
        LocalAuraAcquisitionRoute::StableDirect | LocalAuraAcquisitionRoute::A2dpWakeThenStable => {
            health.aura_transport == LocalAuraTransport::BrEdr
        }
        LocalAuraAcquisitionRoute::FreshLe => health.aura_transport == LocalAuraTransport::Le,
        LocalAuraAcquisitionRoute::Unresolved => true,
    }
}

fn valid_member_projection(member: &LocalPairMember) -> bool {
    !member.channels.is_empty()
        && member.channels.len() <= 6
        && member.channels.iter().enumerate().all(|(index, channel)| {
            (channel != &LocalPairMemberChannel::Unknown || member.channels.len() == 1)
                && !member.channels[..index].contains(channel)
        })
}

fn remaining(deadline: Instant) -> Result<Duration, LocalClientError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(LocalClientError::TimedOut)
}

fn write_all_before(
    stream: &mut TcpStream,
    mut payload: &[u8],
    deadline: Instant,
) -> Result<(), LocalClientError> {
    while !payload.is_empty() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(map_io_error)?;
        match stream.write(payload) {
            Ok(0) => return Err(LocalClientError::Unavailable),
            Ok(written) => payload = &payload[written..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(map_io_error(error)),
        }
    }
    Ok(())
}

fn read_to_end_before(
    stream: &mut TcpStream,
    deadline: Instant,
) -> Result<Vec<u8>, LocalClientError> {
    let mut payload = Vec::new();
    let mut chunk = [0_u8; 2048];
    loop {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(map_io_error)?;
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(payload),
            Ok(count) => {
                if payload.len().saturating_add(count) as u64 > MAX_RESPONSE_BYTES {
                    return Err(LocalClientError::ResponseTooLarge);
                }
                payload.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(map_io_error(error)),
        }
    }
}

struct LocalHttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Drop for LocalHttpResponse {
    fn drop(&mut self) {
        for value in self.headers.values_mut() {
            value.zeroize();
        }
        self.body.zeroize();
    }
}

fn parse_response(payload: &[u8]) -> Result<LocalHttpResponse, LocalClientError> {
    let head_end = find_subslice(payload, b"\r\n\r\n").ok_or(LocalClientError::InvalidResponse)?;
    if head_end > MAX_HEADER_BYTES {
        return Err(LocalClientError::ResponseTooLarge);
    }
    let head =
        std::str::from_utf8(&payload[..head_end]).map_err(|_| LocalClientError::InvalidResponse)?;
    if !head
        .bytes()
        .all(|byte| matches!(byte, b'\t' | b'\r' | b'\n' | 0x20..=0x7e))
    {
        return Err(LocalClientError::InvalidResponse);
    }
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(LocalClientError::InvalidResponse)?;
    let mut status_parts = status_line.splitn(3, ' ');
    if status_parts.next() != Some("HTTP/1.1") {
        return Err(LocalClientError::InvalidResponse);
    }
    let status_text = status_parts
        .next()
        .ok_or(LocalClientError::InvalidResponse)?;
    if status_text.len() != 3 || !status_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(LocalClientError::InvalidResponse);
    }
    let status = status_text
        .parse::<u16>()
        .map_err(|_| LocalClientError::InvalidResponse)?;
    if status_parts.next().is_none() {
        return Err(LocalClientError::InvalidResponse);
    }
    let mut headers = BTreeMap::new();
    for (count, line) in lines.enumerate() {
        if count >= MAX_HEADER_COUNT || line.is_empty() || line.starts_with([' ', '\t']) {
            return Err(LocalClientError::InvalidResponse);
        }
        let (name, raw_value) = line
            .split_once(':')
            .ok_or(LocalClientError::InvalidResponse)?;
        let name = name.to_ascii_lowercase();
        let value = raw_value.trim_matches([' ', '\t']);
        if !valid_header_name(&name)
            || !valid_header_value(value)
            || headers.insert(name, value.to_string()).is_some()
        {
            return Err(LocalClientError::InvalidResponse);
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err(LocalClientError::InvalidResponse);
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or(LocalClientError::InvalidResponse)?;
    let body = &payload[head_end + 4..];
    if body.len() != content_length || body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(LocalClientError::InvalidResponse);
    }
    Ok(LocalHttpResponse {
        status,
        headers,
        body: body.to_vec(),
    })
}

fn map_connect_error(error: std::io::Error) -> LocalClientError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => LocalClientError::TimedOut,
        _ => LocalClientError::Unavailable,
    }
}

fn map_io_error(error: std::io::Error) -> LocalClientError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => LocalClientError::TimedOut,
        _ => LocalClientError::Unavailable,
    }
}

fn valid_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| matches!(byte, b'\t' | 0x20..=0x7e))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    const TOKEN: &str = concat!(
        "5a5a5a5a5a5a5a5a",
        "5a5a5a5a5a5a5a5a",
        "5a5a5a5a5a5a5a5a",
        "5a5a5a5a5a5a5a5a"
    );
    const FIXTURE_IO_TIMEOUT: Duration = Duration::from_secs(2);

    #[test]
    fn action_deadline_covers_the_bounded_native_transaction() {
        assert_eq!(ACTION_TIMEOUT, Duration::from_secs(ACTION_TIMEOUT_SECONDS));
    }

    #[test]
    fn acquisition_route_must_match_the_sanitized_transport() {
        let health = |aura_transport, aura_acquisition_route| LocalHealth {
            lifecycle: LocalLifecycle::Ready,
            level: LocalHealthLevel::Healthy,
            reported_error: false,
            aura_transport,
            aura_acquisition_route,
        };
        assert!(valid_health_projection(health(
            LocalAuraTransport::BrEdr,
            LocalAuraAcquisitionRoute::StableDirect,
        )));
        assert!(valid_health_projection(health(
            LocalAuraTransport::BrEdr,
            LocalAuraAcquisitionRoute::A2dpWakeThenStable,
        )));
        assert!(valid_health_projection(health(
            LocalAuraTransport::Le,
            LocalAuraAcquisitionRoute::FreshLe,
        )));
        assert!(valid_health_projection(health(
            LocalAuraTransport::Unknown,
            LocalAuraAcquisitionRoute::Unresolved,
        )));
        assert!(!valid_health_projection(health(
            LocalAuraTransport::Le,
            LocalAuraAcquisitionRoute::StableDirect,
        )));
    }

    fn response(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn read_request(stream: &mut TcpStream) -> String {
        stream.set_read_timeout(Some(FIXTURE_IO_TIMEOUT)).unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let count = stream.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(head_end) = find_subslice(&bytes, b"\r\n\r\n") {
                let head = String::from_utf8(bytes[..head_end].to_vec()).unwrap();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap();
                if bytes.len() >= head_end + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    fn accept_before(listener: &TcpListener) -> TcpStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + FIXTURE_IO_TIMEOUT;
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "loopback fixture did not receive the expected request"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("loopback fixture accept failed: {error}"),
            }
        }
    }

    #[test]
    fn mutation_gets_cookie_and_revision_then_posts_exactly_once() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let replies = [
                response(
                    "200 OK",
                    &format!("Set-Cookie: {CSRF_COOKIE}={TOKEN}; Path=/; SameSite=Strict\r\n"),
                    "page",
                ),
                response(
                    "200 OK",
                    "Content-Type: application/json\r\nETag: \"7\"\r\n",
                    r#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"JBL Authentics 300","verification":"verified","channels":["stereo"]},{"name":"Aura Studio 5","verification":"verified","channels":["mono"]}],"managed_state":"ready","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":7,"last_action":null}"#,
                ),
                response(
                    "200 OK",
                    "Content-Type: application/json\r\nETag: \"9\"\r\n",
                    r#"{"action":"start","outcome":"accepted","managed_state":"linked","evidence":"lifecycle_acknowledgement","failure":null,"revision":9}"#,
                ),
            ];
            for reply in replies {
                let mut stream = accept_before(&listener);
                let request = read_request(&mut stream);
                requests_tx.send(request).unwrap();
                stream.write_all(&reply).unwrap();
            }
        });

        let result = LocalServiceClient::for_test(address).start().unwrap();
        assert_eq!(result.action, LocalActionName::Start);
        let requests: Vec<_> = (0..3)
            .map(|_| requests_rx.recv_timeout(FIXTURE_IO_TIMEOUT).unwrap())
            .collect();
        assert!(requests[0].starts_with("GET / HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("GET /api/status HTTP/1.1\r\n"));
        assert!(requests[2].starts_with("POST /api/start HTTP/1.1\r\n"));
        assert!(requests[2].contains("If-Match: \"7\"\r\n"));
        assert!(requests[2].contains(&format!("X-CSRF-Token: {TOKEN}\r\n")));
        assert!(requests[2].contains(&format!("Cookie: {CSRF_COOKIE}={TOKEN}\r\n")));
        server.join().unwrap();
    }

    #[test]
    fn conflict_is_returned_without_retry() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (count_tx, count_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            for index in 0..3 {
                let mut stream = accept_before(&listener);
                let request = read_request(&mut stream);
                if index == 2 {
                    assert!(request.starts_with("POST /api/stop HTTP/1.1\r\n"));
                }
                let reply = match index {
                    0 => response(
                        "200 OK",
                        &format!("Set-Cookie: {CSRF_COOKIE}={TOKEN}; Path=/\r\n"),
                        "page",
                    ),
                    1 => response(
                        "200 OK",
                        "Content-Type: application/json\r\nETag: \"3\"\r\n",
                        r#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"JBL Authentics 300","verification":"verified","channels":["stereo"]},{"name":"Aura Studio 5","verification":"verified","channels":["mono"]}],"managed_state":"ready","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":3,"last_action":null}"#,
                    ),
                    _ => response(
                        "409 Conflict",
                        "Content-Type: application/json\r\n",
                        r#"{"error":"revision_conflict"}"#,
                    ),
                };
                stream.write_all(&reply).unwrap();
            }
            count_tx.send(3_usize).unwrap();
        });
        assert_eq!(
            LocalServiceClient::for_test(address).stop().unwrap_err(),
            LocalClientError::RevisionConflict
        );
        assert_eq!(count_rx.recv_timeout(FIXTURE_IO_TIMEOUT).unwrap(), 3);
        server.join().unwrap();
    }

    #[test]
    fn response_parser_rejects_duplicates_chunking_and_overlong_bodies() {
        let duplicate = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n";
        assert!(matches!(
            parse_response(duplicate),
            Err(LocalClientError::InvalidResponse)
        ));
        let chunked = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(matches!(
            parse_response(chunked),
            Err(LocalClientError::InvalidResponse)
        ));
        let mismatch = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}x";
        assert!(matches!(
            parse_response(mismatch),
            Err(LocalClientError::InvalidResponse)
        ));
    }

    #[test]
    fn trickle_response_cannot_extend_the_absolute_deadline() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut stream = accept_before(&listener);
            let _request = read_request(&mut stream);
            let reply = response("200 OK", "", r#"{"status":"ready"}"#);
            for byte in reply {
                if stream.write_all(&[byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
        let started = Instant::now();
        let result = LocalServiceClient::for_test(address).request(
            "GET",
            "/healthz",
            &[],
            &[],
            Duration::from_millis(150),
        );
        assert!(matches!(result, Err(LocalClientError::TimedOut)));
        assert!(started.elapsed() < Duration::from_millis(600));
        server.join().unwrap();
    }

    #[test]
    fn fixture_responses_parse_and_deserialize_as_expected() {
        let page = response(
            "200 OK",
            &format!("Set-Cookie: {CSRF_COOKIE}={TOKEN}; Path=/\r\n"),
            "page",
        );
        let parsed = parse_response(&page).unwrap();
        assert_eq!(
            parsed.headers["set-cookie"].split(';').next().unwrap(),
            format!("{CSRF_COOKIE}={TOKEN}")
        );
        let status = response(
            "200 OK",
            "Content-Type: application/json\r\nETag: \"7\"\r\n",
            r#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"JBL Authentics 300","verification":"verified","channels":["stereo"]},{"name":"Aura Studio 5","verification":"verified","channels":["mono"]}],"managed_state":"ready","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":7,"last_action":null}"#,
        );
        let parsed = parse_response(&status).unwrap();
        let status: LocalStatus = serde_json::from_slice(&parsed.body).unwrap();
        assert_eq!(status.revision, 7);
        let action = response(
            "200 OK",
            "Content-Type: application/json\r\nETag: \"9\"\r\n",
            r#"{"action":"start","outcome":"accepted_unconfirmed","managed_state":"linked","evidence":"broadcast_acknowledgement_only","failure":null,"revision":9}"#,
        );
        let parsed = parse_response(&action).unwrap();
        let action: LocalActionResult = serde_json::from_slice(&parsed.body).unwrap();
        assert_eq!(action.action, LocalActionName::Start);
        assert_eq!(action.outcome, LocalActionOutcome::AcceptedUnconfirmed);
        assert_eq!(
            action.evidence,
            Some(LocalActionEvidence::BroadcastAcknowledgementOnly)
        );
        assert!(action.succeeded());
        assert_eq!(parsed.headers["etag"], "\"9\"");

        for (label, expected) in [
            (
                "aura_invalid_configuration",
                LocalFailure::AuraInvalidConfiguration,
            ),
            (
                "aura_runtime_unavailable",
                LocalFailure::AuraRuntimeUnavailable,
            ),
            (
                "aura_transport_not_ready",
                LocalFailure::AuraTransportNotReady,
            ),
            (
                "aura_notification_queue_invalid",
                LocalFailure::AuraNotificationQueueInvalid,
            ),
            ("aura_disconnect_failed", LocalFailure::AuraDisconnectFailed),
            ("aura_write_unknown", LocalFailure::AuraWriteUnknown),
            ("aura_ack_timeout", LocalFailure::AuraAckTimeout),
            (
                "aura_ack_channel_closed",
                LocalFailure::AuraAckChannelClosed,
            ),
            ("aura_unexpected_ack", LocalFailure::AuraUnexpectedAck),
            (
                "jbl_enter_outcome_unknown",
                LocalFailure::JblEnterOutcomeUnknown,
            ),
            (
                "jbl_exit_outcome_unknown",
                LocalFailure::JblExitOutcomeUnknown,
            ),
            (
                "jbl_broadcast_result_timed_out",
                LocalFailure::JblBroadcastResultTimedOut,
            ),
            (
                "jbl_broadcast_result_unavailable",
                LocalFailure::JblBroadcastResultUnavailable,
            ),
            (
                "jbl_broadcast_result_rejected",
                LocalFailure::JblBroadcastResultRejected,
            ),
            (
                "aura_start_outcome_unknown",
                LocalFailure::AuraStartOutcomeUnknown,
            ),
        ] {
            let body = format!(
                r#"{{"action":"start","outcome":"outcome_unknown","managed_state":"unknown","evidence":null,"failure":"{label}","revision":10}}"#
            );
            let action: LocalActionResult = serde_json::from_str(&body).unwrap();
            assert_eq!(action.failure, Some(expected));
        }
    }

    #[test]
    fn status_projection_rejects_unfixed_members_and_inconsistent_evidence() {
        let malicious = br#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"private device name","verification":"verified","channels":["stereo"]},{"name":"Aura Studio 5","verification":"verified","channels":["mono"]}],"managed_state":"ready","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":7,"last_action":null}"#;
        assert!(serde_json::from_slice::<LocalStatus>(malicious).is_err());

        let swapped = br#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"Aura Studio 5","verification":"verified","channels":["mono"]},{"name":"JBL Authentics 300","verification":"verified","channels":["stereo"]}],"managed_state":"ready","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":7,"last_action":null}"#;
        let swapped: LocalStatus = serde_json::from_slice(swapped).unwrap();
        assert!(!valid_status_projection(&swapped));

        let future_action = br#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"JBL Authentics 300","verification":"verified","channels":["stereo"]},{"name":"Aura Studio 5","verification":"verified","channels":["mono"]}],"managed_state":"ready","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":7,"last_action":{"action":"start","outcome":"accepted","evidence":"broadcast_business_notification","failure":null,"revision":8,"age_ms":0}}"#;
        let future_action: LocalStatus = serde_json::from_slice(future_action).unwrap();
        assert!(!valid_status_projection(&future_action));

        let honest_unconfirmed = br#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"JBL Authentics 300","verification":"verified","channels":["stereo"]},{"name":"Aura Studio 5","verification":"verified","channels":["mono"]}],"managed_state":"linked","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":9,"last_action":{"action":"start","outcome":"accepted_unconfirmed","evidence":"broadcast_acknowledgement_only","failure":null,"revision":9,"age_ms":3}}"#;
        let honest_unconfirmed: LocalStatus = serde_json::from_slice(honest_unconfirmed).unwrap();
        assert!(valid_status_projection(&honest_unconfirmed));

        let dishonest_ack_as_accepted = br#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"JBL Authentics 300","verification":"verified","channels":["stereo"]},{"name":"Aura Studio 5","verification":"verified","channels":["mono"]}],"managed_state":"linked","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":9,"last_action":{"action":"start","outcome":"accepted","evidence":"broadcast_acknowledgement_only","failure":null,"revision":9,"age_ms":3}}"#;
        let dishonest_ack_as_accepted: LocalStatus =
            serde_json::from_slice(dishonest_ack_as_accepted).unwrap();
        assert!(!valid_status_projection(&dishonest_ack_as_accepted));
    }

    #[test]
    fn action_projection_enforces_closed_outcome_evidence_failure_and_state() {
        let honest = LocalActionResult {
            action: LocalActionName::Start,
            outcome: LocalActionOutcome::AcceptedUnconfirmed,
            managed_state: LocalManagedState::Linked,
            evidence: Some(LocalActionEvidence::BroadcastAcknowledgementOnly),
            failure: None,
            revision: 2,
        };
        assert!(valid_action_projection(&honest));

        let dishonest_evidence = LocalActionResult {
            outcome: LocalActionOutcome::Accepted,
            ..honest
        };
        assert!(!valid_action_projection(&dishonest_evidence));

        let dishonest_state = LocalActionResult {
            managed_state: LocalManagedState::Ready,
            ..honest
        };
        assert!(!valid_action_projection(&dishonest_state));

        let honest_unknown = LocalActionResult {
            action: LocalActionName::Stop,
            outcome: LocalActionOutcome::OutcomeUnknown,
            managed_state: LocalManagedState::Unknown,
            evidence: None,
            failure: Some(LocalFailure::JblExitOutcomeUnknown),
            revision: 4,
        };
        assert!(valid_action_projection(&honest_unknown));

        let dishonest_unknown_without_failure = LocalActionResult {
            failure: None,
            ..honest_unknown
        };
        assert!(!valid_action_projection(&dishonest_unknown_without_failure));
    }

    #[test]
    fn mutation_rejects_an_action_response_that_does_not_advance_revision() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let replies = [
                response(
                    "200 OK",
                    &format!("Set-Cookie: {CSRF_COOKIE}={TOKEN}; Path=/\r\n"),
                    "page",
                ),
                response(
                    "200 OK",
                    "Content-Type: application/json\r\nETag: \"7\"\r\n",
                    r#"{"backend":"native_pair","pair_configuration":"ready","members":[{"name":"JBL Authentics 300","verification":"verified","channels":["stereo"]},{"name":"Aura Studio 5","verification":"verified","channels":["mono"]}],"managed_state":"ready","backend_health":null,"unresolved_action":false,"consecutive_failures":0,"revision":7,"last_action":null}"#,
                ),
                response(
                    "200 OK",
                    "Content-Type: application/json\r\nETag: \"7\"\r\n",
                    r#"{"action":"start","outcome":"accepted","managed_state":"linked","failure":null,"revision":7}"#,
                ),
            ];
            for reply in replies {
                let mut stream = accept_before(&listener);
                let _request = read_request(&mut stream);
                stream.write_all(&reply).unwrap();
            }
        });
        assert!(matches!(
            LocalServiceClient::for_test(address).start(),
            Err(LocalClientError::InvalidResponse)
        ));
        server.join().unwrap();
    }
}
