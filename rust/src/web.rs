//! Minimal, loopback-only HTTP boundary for the Play Together controller.
//!
//! This module intentionally contains no socket listener and no device/backend
//! handle.  A future service loop can feed one complete HTTP/1.1 request into
//! [`WebApp::handle`], but every mutation still has to cross the single-owner
//! [`WebActor`] compare-and-mutate boundary.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::backend::{
    AuraAcquisitionRoute, AuraControlTransport, PairBackendEvidence, PairBackendKind, PairHealth,
    PairHealthLevel, PairLifecycle,
};
use crate::capability::{Capability, CapabilityMaturity};
use crate::controller::{
    ControllerAction, ControllerActionOutcome, ControllerActionResult, ControllerFailure,
    ControllerStatus, LastActionStatus, ManagedLiveState, PairConfigurationState,
    PairMemberChannel, PairMemberName, PairMemberStatus, PairMemberVerification,
};
use crate::eq::EqPresetTarget;
use crate::inspection::InspectionSnapshot;
use crate::media::{AudioSourceTarget, MediaStatus, MuteTarget};
use crate::web_device::{
    DirectActionOutcome, DirectActionResult, DirectFailure, DirectMutation, DirectSnapshot,
};

pub const DEFAULT_WEB_PORT: u16 = 8096;
pub(crate) const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADER_COUNT: usize = 64;
pub(crate) const MAX_BODY_BYTES: usize = 4 * 1024;
pub(crate) const MAX_REQUEST_BYTES: usize = MAX_HEADER_BYTES + 4 + MAX_BODY_BYTES;
const CSRF_COOKIE: &str = "jbl_aura_csrf";
const CSRF_HEADER: &str = "x-csrf-token";

/// The sole mutations exposed by the P0 Web surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebMutation {
    Start,
    Stop,
    /// Advanced local recovery.  It is intentionally absent from the HTML UI.
    RecoverStop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JblWebAction {
    Volume,
    Mute,
    Source,
    EqPreset,
}

/// Returned by the actor when its revision changed before the mutation could
/// be applied.  It intentionally contains no diagnostic or device data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionConflict;

/// Serialized access to the one controller actor.
///
/// `mutate_if_revision` must compare `expected_revision` and perform the
/// mutation as one actor operation.  It must return `RevisionConflict` without
/// touching a backend when the revision differs.  The HTTP layer never owns or
/// calls a device backend directly.
pub trait WebActor {
    fn status(&mut self) -> ControllerStatus;

    fn mutate_if_revision(
        &mut self,
        expected_revision: u64,
        mutation: WebMutation,
    ) -> Result<ControllerActionResult, RevisionConflict>;

    fn direct_snapshot(&mut self) -> Result<DirectSnapshot, DirectFailure> {
        Err(DirectFailure::Unavailable)
    }

    fn mutate_direct_if_revision(
        &mut self,
        expected_revision: u64,
        _mutation: DirectMutation,
    ) -> Result<DirectActionResult, RevisionConflict> {
        Ok(DirectActionResult {
            outcome: DirectActionOutcome::RejectedBeforeSend,
            observation: None,
            failure: Some(DirectFailure::Unavailable),
            revision: expected_revision,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebConfigError {
    NonLoopbackBind,
    InvalidPort,
    InvalidCsrfToken,
}

/// Security inputs for a future listener.  Construction rejects every
/// non-loopback bind address; there is no public escape hatch.
pub struct WebSecurity {
    bind_addr: SocketAddr,
    csrf_token: String,
}

impl WebSecurity {
    /// The default is IPv4 loopback on port 8096.  The caller supplies 32 bytes
    /// from an operating-system CSPRNG; the all-zero value is rejected.
    pub fn loopback(csrf_token: [u8; 32]) -> Result<Self, WebConfigError> {
        Self::new(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_WEB_PORT),
            csrf_token,
        )
    }

    pub fn new(bind_addr: SocketAddr, csrf_token: [u8; 32]) -> Result<Self, WebConfigError> {
        if !matches!(
            bind_addr.ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST) | IpAddr::V6(Ipv6Addr::LOCALHOST)
        ) {
            return Err(WebConfigError::NonLoopbackBind);
        }
        if bind_addr.port() == 0 {
            return Err(WebConfigError::InvalidPort);
        }
        if csrf_token.iter().all(|byte| *byte == 0) {
            return Err(WebConfigError::InvalidCsrfToken);
        }
        let mut encoded = String::with_capacity(64);
        for byte in csrf_token {
            use std::fmt::Write as _;
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        Ok(Self {
            bind_addr,
            csrf_token: encoded,
        })
    }

    pub const fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    fn port(&self) -> u16 {
        self.bind_addr.port()
    }
}

impl Drop for WebSecurity {
    fn drop(&mut self) {
        self.csrf_token.zeroize();
    }
}

/// A complete, already-serialized HTTP response.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    pub const fn status(&self) -> u16 {
        self.status
    }

    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Serialize as one HTTP/1.1 response.  Responses always close the
    /// connection, so a caller never needs to support pipelining.
    pub fn to_http1_bytes(&self) -> Vec<u8> {
        let reason = reason_phrase(self.status);
        let mut output = format!("HTTP/1.1 {} {}\r\n", self.status, reason).into_bytes();
        for (name, value) in &self.headers {
            output.extend_from_slice(name.as_bytes());
            output.extend_from_slice(b": ");
            output.extend_from_slice(value.as_bytes());
            output.extend_from_slice(b"\r\n");
        }
        output.extend_from_slice(b"\r\n");
        output.extend_from_slice(&self.body);
        output
    }

    #[cfg(test)]
    fn header(&self, wanted: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value.as_str())
    }
}

/// Pure request parser/handler.  It opens no socket and performs no device I/O
/// except through the supplied actor trait.
pub struct WebApp<A> {
    actor: A,
    security: WebSecurity,
}

impl<A: WebActor> WebApp<A> {
    pub const fn new(actor: A, security: WebSecurity) -> Self {
        Self { actor, security }
    }

    pub fn handle(&mut self, raw_request: &[u8]) -> HttpResponse {
        let request = match parse_request(raw_request) {
            Ok(request) => request,
            Err(error) => return error_response(error),
        };

        let host = match request.single_header("host") {
            Some(value) => match LoopbackAuthority::parse(value, self.security.port()) {
                Some(authority) => authority,
                None => return error_response(RequestError::ForbiddenHost),
            },
            None => return error_response(RequestError::ForbiddenHost),
        };

        if let Some(origin) = request.single_header("origin") {
            if !origin_is_same(origin, host) {
                return error_response(RequestError::ForbiddenOrigin);
            }
        }

        match (request.method, request.target.as_str()) {
            (Method::Get, "/") => self.page_response(),
            (Method::Get, "/api/status") => {
                let status = self.actor.status();
                status_response(status)
            }
            (Method::Get, "/api/jbl/status") => match self.actor.direct_snapshot() {
                Ok(snapshot) => jbl_status_response(snapshot),
                Err(_) => jbl_unavailable_response(),
            },
            (Method::Get, "/healthz") => readiness_response(),
            (Method::Post, "/api/start") => {
                self.mutation_response(&request, WebMutation::Start, MutationBody::EmptyObject)
            }
            (Method::Post, "/api/stop") => {
                self.mutation_response(&request, WebMutation::Stop, MutationBody::EmptyObject)
            }
            (Method::Post, "/internal/recover-stop") => self.mutation_response(
                &request,
                WebMutation::RecoverStop,
                MutationBody::RecoverStopConfirmation,
            ),
            (Method::Post, "/api/jbl/volume") => {
                self.jbl_mutation_response(&request, JblMutationBody::Volume)
            }
            (Method::Post, "/api/jbl/mute") => {
                self.jbl_mutation_response(&request, JblMutationBody::Mute)
            }
            (Method::Post, "/api/jbl/source") => {
                self.jbl_mutation_response(&request, JblMutationBody::Source)
            }
            (Method::Post, "/api/jbl/eq-preset") => {
                self.jbl_mutation_response(&request, JblMutationBody::EqPreset)
            }
            (Method::Get | Method::Post, _) => error_response(RequestError::NotFound),
            (Method::Other, _) => error_response(RequestError::MethodNotAllowed),
        }
    }

    pub fn into_actor(self) -> A {
        self.actor
    }

    fn page_response(&self) -> HttpResponse {
        let page = PAGE_HTML.replace("__CSP_NONCE__", &self.security.csrf_token);
        let cookie = format!(
            "{CSRF_COOKIE}={}; Path=/; SameSite=Strict",
            self.security.csrf_token
        );
        response(
            200,
            "text/html; charset=utf-8",
            page.into_bytes(),
            Some(&self.security.csrf_token),
            [("Set-Cookie", cookie)],
        )
    }

    fn mutation_response(
        &mut self,
        request: &ParsedRequest,
        mutation: WebMutation,
        body_rule: MutationBody,
    ) -> HttpResponse {
        let expected_revision =
            match self.validated_mutation_revision(request, body_rule.matches(&request.body)) {
                Ok(revision) => revision,
                Err(error) => return error_response(error),
            };

        match self.actor.mutate_if_revision(expected_revision, mutation) {
            Ok(result) => action_response(result),
            Err(RevisionConflict) => error_response(RequestError::RevisionConflict),
        }
    }

    fn jbl_mutation_response(
        &mut self,
        request: &ParsedRequest,
        body_rule: JblMutationBody,
    ) -> HttpResponse {
        let Some(mutation) = body_rule.parse(&request.body) else {
            return error_response(RequestError::InvalidJsonBody);
        };
        let expected_revision = match self.validated_mutation_revision(request, true) {
            Ok(revision) => revision,
            Err(error) => return error_response(error),
        };
        match self
            .actor
            .mutate_direct_if_revision(expected_revision, mutation)
        {
            Ok(result) => jbl_action_response(body_rule.action(), result),
            Err(RevisionConflict) => error_response(RequestError::RevisionConflict),
        }
    }

    fn validated_mutation_revision(
        &self,
        request: &ParsedRequest,
        body_valid: bool,
    ) -> Result<u64, RequestError> {
        if request.single_header("origin").is_none() {
            return Err(RequestError::ForbiddenOrigin);
        }
        if request.single_header("content-type") != Some("application/json") {
            return Err(RequestError::UnsupportedMediaType);
        }
        let Some(content_length) = request.content_length else {
            return Err(RequestError::LengthRequired);
        };
        if content_length > MAX_BODY_BYTES {
            return Err(RequestError::PayloadTooLarge);
        }
        if !body_valid {
            return Err(RequestError::InvalidJsonBody);
        }
        if !self.valid_csrf(request) {
            return Err(RequestError::CsrfRejected);
        }
        request
            .single_header("if-match")
            .and_then(parse_strong_revision)
            .ok_or(RequestError::PreconditionRequired)
    }

    fn valid_csrf(&self, request: &ParsedRequest) -> bool {
        let Some(header_token) = request.single_header(CSRF_HEADER) else {
            return false;
        };
        let Some(cookie_header) = request.single_header("cookie") else {
            return false;
        };
        let Some(cookie_token) = csrf_cookie_value(cookie_header) else {
            return false;
        };

        constant_time_eq(header_token.as_bytes(), cookie_token.as_bytes())
            & constant_time_eq(header_token.as_bytes(), self.security.csrf_token.as_bytes())
            & constant_time_eq(cookie_token.as_bytes(), self.security.csrf_token.as_bytes())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MutationBody {
    EmptyObject,
    RecoverStopConfirmation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JblMutationBody {
    Volume,
    Mute,
    Source,
    EqPreset,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JblVolumeRequest {
    value: u8,
    confirm: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JblMuteRequest {
    state: String,
    confirm: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JblTargetRequest {
    target: String,
    confirm: String,
}

impl JblMutationBody {
    fn parse(self, body: &[u8]) -> Option<DirectMutation> {
        match self {
            Self::Volume => {
                let request = serde_json::from_slice::<JblVolumeRequest>(body).ok()?;
                (request.confirm == "volume-set" && request.value <= 9)
                    .then_some(DirectMutation::Volume(request.value))
            }
            Self::Mute => {
                let request = serde_json::from_slice::<JblMuteRequest>(body).ok()?;
                if request.confirm != "mute-set" {
                    return None;
                }
                match request.state.as_str() {
                    "on" => Some(DirectMutation::Mute(MuteTarget::On)),
                    "off" => Some(DirectMutation::Mute(MuteTarget::Off)),
                    _ => None,
                }
            }
            Self::Source => {
                let request = serde_json::from_slice::<JblTargetRequest>(body).ok()?;
                if request.confirm != "source-set" {
                    return None;
                }
                match request.target.as_str() {
                    "bluetooth" => Some(DirectMutation::Source(AudioSourceTarget::Bluetooth)),
                    "aux" => Some(DirectMutation::Source(AudioSourceTarget::AuxIn)),
                    "usb" => Some(DirectMutation::Source(AudioSourceTarget::UsbPlayback)),
                    _ => None,
                }
            }
            Self::EqPreset => {
                let request = serde_json::from_slice::<JblTargetRequest>(body).ok()?;
                if request.confirm != "eq-preset-set" {
                    return None;
                }
                match request.target.as_str() {
                    "signature" => Some(DirectMutation::EqPreset(EqPresetTarget::Signature)),
                    "vocal" => Some(DirectMutation::EqPreset(EqPresetTarget::Vocal)),
                    "energetic" => Some(DirectMutation::EqPreset(EqPresetTarget::Energetic)),
                    "chill" => Some(DirectMutation::EqPreset(EqPresetTarget::Chill)),
                    _ => None,
                }
            }
        }
    }

    const fn action(self) -> JblWebAction {
        match self {
            Self::Volume => JblWebAction::Volume,
            Self::Mute => JblWebAction::Mute,
            Self::Source => JblWebAction::Source,
            Self::EqPreset => JblWebAction::EqPreset,
        }
    }
}

impl MutationBody {
    fn matches(self, body: &[u8]) -> bool {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
            return false;
        };
        let Some(object) = value.as_object() else {
            return false;
        };
        match self {
            Self::EmptyObject => object.is_empty(),
            Self::RecoverStopConfirmation => {
                object.len() == 1
                    && object.get("confirm").and_then(serde_json::Value::as_str)
                        == Some("recover-stop")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Get,
    Post,
    Other,
}

struct ParsedRequest {
    method: Method,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    content_length: Option<usize>,
}

impl ParsedRequest {
    fn single_header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestError {
    BadRequest,
    ForbiddenHost,
    ForbiddenOrigin,
    CsrfRejected,
    NotFound,
    MethodNotAllowed,
    RevisionConflict,
    LengthRequired,
    PayloadTooLarge,
    UnsupportedMediaType,
    PreconditionRequired,
    InvalidJsonBody,
    HttpVersionNotSupported,
}

/// Minimal framing result used by the loopback server before it hands a byte
/// slice to the stricter parser above.  `Rejected` is terminal and must never
/// be retried on the same connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestFrame {
    Incomplete,
    Complete,
    Rejected,
}

/// Determine whether one complete request is buffered without waiting for
/// EOF.  This duplicates only the Content-Length fields needed for safe
/// framing; [`parse_request`] remains the authority for all HTTP semantics.
pub(crate) fn request_frame(raw: &[u8]) -> RequestFrame {
    if raw.len() > MAX_REQUEST_BYTES {
        return RequestFrame::Rejected;
    }
    let Some(head_end) = find_subslice(raw, b"\r\n\r\n") else {
        return if raw.len() > MAX_HEADER_BYTES + 4 {
            RequestFrame::Rejected
        } else {
            RequestFrame::Incomplete
        };
    };
    if head_end > MAX_HEADER_BYTES {
        return RequestFrame::Rejected;
    }
    let Ok(head) = std::str::from_utf8(&raw[..head_end]) else {
        return RequestFrame::Rejected;
    };
    let mut lines = head.split("\r\n");
    let Some(request_line) = lines.next() else {
        return RequestFrame::Rejected;
    };
    let mut parts = request_line.split(' ');
    let Some(method) = parts.next() else {
        return RequestFrame::Rejected;
    };
    if parts.next().is_none() || parts.next().is_none() || parts.next().is_some() {
        return RequestFrame::Rejected;
    }

    let mut content_length = None;
    for line in lines {
        if line.is_empty() || line.starts_with([' ', '\t']) {
            return RequestFrame::Rejected;
        }
        let Some((name, raw_value)) = line.split_once(':') else {
            return RequestFrame::Rejected;
        };
        if name.is_empty() || !name.bytes().all(is_tchar) {
            return RequestFrame::Rejected;
        }
        let value = raw_value.trim_matches([' ', '\t']);
        if value.bytes().any(|byte| !(0x20..=0x7e).contains(&byte)) {
            return RequestFrame::Rejected;
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return RequestFrame::Rejected;
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return RequestFrame::Rejected;
            }
            let Ok(length) = parse_content_length(value) else {
                return RequestFrame::Rejected;
            };
            if length > MAX_BODY_BYTES {
                return RequestFrame::Rejected;
            }
            content_length = Some(length);
        }
    }
    if method == "POST" && content_length.is_none() {
        return RequestFrame::Rejected;
    }
    let expected = match (head_end + 4).checked_add(content_length.unwrap_or(0)) {
        Some(expected) if expected <= MAX_REQUEST_BYTES => expected,
        _ => return RequestFrame::Rejected,
    };
    match raw.len().cmp(&expected) {
        std::cmp::Ordering::Less => RequestFrame::Incomplete,
        std::cmp::Ordering::Equal => RequestFrame::Complete,
        std::cmp::Ordering::Greater => RequestFrame::Rejected,
    }
}

fn parse_request(raw: &[u8]) -> Result<ParsedRequest, RequestError> {
    let Some(head_end) = find_subslice(raw, b"\r\n\r\n") else {
        return Err(if raw.len() > MAX_HEADER_BYTES {
            RequestError::PayloadTooLarge
        } else {
            RequestError::BadRequest
        });
    };
    if head_end > MAX_HEADER_BYTES {
        return Err(RequestError::PayloadTooLarge);
    }
    let head = &raw[..head_end];
    if head
        .iter()
        .any(|byte| !matches!(*byte, b'\t' | 0x20..=0x7e | b'\r' | b'\n'))
    {
        return Err(RequestError::BadRequest);
    }
    let head_text = std::str::from_utf8(head).map_err(|_| RequestError::BadRequest)?;
    let mut lines = head_text.split("\r\n");
    let request_line = lines.next().ok_or(RequestError::BadRequest)?;
    let mut request_parts = request_line.split(' ');
    let method_text = request_parts.next().ok_or(RequestError::BadRequest)?;
    let target = request_parts.next().ok_or(RequestError::BadRequest)?;
    let version = request_parts.next().ok_or(RequestError::BadRequest)?;
    if request_parts.next().is_some()
        || method_text.is_empty()
        || target.is_empty()
        || version.is_empty()
    {
        return Err(RequestError::BadRequest);
    }
    if version != "HTTP/1.1" {
        return Err(RequestError::HttpVersionNotSupported);
    }
    if !method_text.bytes().all(is_tchar) {
        return Err(RequestError::BadRequest);
    }
    let method = match method_text {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => Method::Other,
    };
    if !target.starts_with('/')
        || target.starts_with("//")
        || target.contains('?')
        || target.contains('#')
        || target.bytes().any(|byte| !(0x21..=0x7e).contains(&byte))
    {
        return Err(RequestError::BadRequest);
    }

    let mut headers = BTreeMap::new();
    let mut count = 0usize;
    for line in lines {
        count = count.saturating_add(1);
        if count > MAX_HEADER_COUNT || line.is_empty() || line.starts_with([' ', '\t']) {
            return Err(RequestError::BadRequest);
        }
        let Some(colon) = line.find(':') else {
            return Err(RequestError::BadRequest);
        };
        let (raw_name, raw_value) = line.split_at(colon);
        if raw_name.is_empty() || !raw_name.bytes().all(is_tchar) {
            return Err(RequestError::BadRequest);
        }
        let value = raw_value[1..].trim_matches([' ', '\t']);
        if value.bytes().any(|byte| !(0x20..=0x7e).contains(&byte)) {
            return Err(RequestError::BadRequest);
        }
        let name = raw_name.to_ascii_lowercase();
        if headers.insert(name, value.to_string()).is_some() {
            // Reject every duplicate header rather than maintaining an
            // incomplete list of headers whose merging is dangerous.
            return Err(RequestError::BadRequest);
        }
    }

    for forbidden in [
        "transfer-encoding",
        "trailer",
        "upgrade",
        "proxy-connection",
    ] {
        if headers.contains_key(forbidden) {
            return Err(RequestError::BadRequest);
        }
    }

    let content_length = match headers.get("content-length") {
        Some(value) => Some(parse_content_length(value)?),
        None => None,
    };
    if method == Method::Post && content_length.is_none() {
        return Err(RequestError::LengthRequired);
    }
    let body = &raw[head_end + 4..];
    match content_length {
        Some(length) if length > MAX_BODY_BYTES => {
            return Err(RequestError::PayloadTooLarge);
        }
        Some(length) if body.len() != length => return Err(RequestError::BadRequest),
        None if !body.is_empty() => return Err(RequestError::BadRequest),
        _ => {}
    }
    if method == Method::Get && content_length.unwrap_or(0) != 0 {
        return Err(RequestError::BadRequest);
    }

    Ok(ParsedRequest {
        method,
        target: target.to_string(),
        headers,
        body: body.to_vec(),
        content_length,
    })
}

fn parse_content_length(value: &str) -> Result<usize, RequestError> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(RequestError::BadRequest);
    }
    value
        .parse::<usize>()
        .map_err(|_| RequestError::PayloadTooLarge)
}

fn parse_strong_revision(value: &str) -> Option<u64> {
    let digits = value.strip_prefix('"')?.strip_suffix('"')?;
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    digits.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthorityHost {
    Localhost,
    Ipv4Loopback,
    Ipv6Loopback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoopbackAuthority {
    host: AuthorityHost,
    effective_port: u16,
}

impl LoopbackAuthority {
    fn parse(value: &str, configured_port: u16) -> Option<Self> {
        if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return None;
        }
        let (host, port) = if let Some(rest) = value.strip_prefix('[') {
            let end = rest.find(']')?;
            if &rest[..end] != "::1" {
                return None;
            }
            let suffix = &rest[end + 1..];
            let port = if suffix.is_empty() {
                None
            } else {
                Some(parse_port(suffix.strip_prefix(':')?)?)
            };
            (AuthorityHost::Ipv6Loopback, port)
        } else {
            let (raw_host, raw_port) = match value.rsplit_once(':') {
                Some((host, port)) => (host, Some(parse_port(port)?)),
                None => (value, None),
            };
            let host = if raw_host.eq_ignore_ascii_case("localhost") {
                AuthorityHost::Localhost
            } else if raw_host == "127.0.0.1" {
                AuthorityHost::Ipv4Loopback
            } else {
                return None;
            };
            (host, raw_port)
        };
        if port.is_some_and(|port| port != configured_port) {
            return None;
        }
        Some(Self {
            host,
            effective_port: port.unwrap_or(80),
        })
    }
}

fn parse_port(value: &str) -> Option<u16> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    let port = value.parse::<u16>().ok()?;
    (port != 0).then_some(port)
}

fn origin_is_same(value: &str, host: LoopbackAuthority) -> bool {
    let Some(authority) = value.strip_prefix("http://") else {
        return false;
    };
    if authority.contains(['/', '?', '#', '@']) {
        return false;
    }
    LoopbackAuthority::parse(authority, host.effective_port).is_some_and(|origin| origin == host)
}

fn csrf_cookie_value(header: &str) -> Option<&str> {
    let mut found = None;
    for field in header.split(';') {
        let field = field.trim_matches([' ', '\t']);
        let (name, value) = field.split_once('=')?;
        if name == CSRF_COOKIE && (found.replace(value).is_some() || value.is_empty()) {
            return None;
        }
    }
    found
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    openssl::memcmp::eq(left, right)
}

#[derive(Serialize)]
struct StatusBody {
    backend: &'static str,
    pair_configuration: &'static str,
    members: [MemberBody; 2],
    managed_state: &'static str,
    backend_health: Option<HealthBody>,
    unresolved_action: bool,
    consecutive_failures: u8,
    revision: u64,
    last_action: Option<LastActionBody>,
}

#[derive(Serialize)]
struct MemberBody {
    name: &'static str,
    verification: &'static str,
    channels: Vec<&'static str>,
}

#[derive(Serialize)]
struct HealthBody {
    lifecycle: &'static str,
    level: &'static str,
    reported_error: bool,
    aura_transport: &'static str,
    aura_acquisition_route: &'static str,
}

#[derive(Serialize)]
struct ActionBody {
    action: &'static str,
    outcome: &'static str,
    managed_state: &'static str,
    evidence: Option<&'static str>,
    failure: Option<&'static str>,
    revision: u64,
}

#[derive(Serialize)]
struct LastActionBody {
    action: &'static str,
    outcome: &'static str,
    evidence: Option<&'static str>,
    failure: Option<&'static str>,
    revision: u64,
    age_ms: u64,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

#[derive(Serialize)]
struct ReadinessBody {
    status: &'static str,
}

#[derive(Serialize)]
struct JblStatusBody<'a> {
    schema_version: u8,
    revision: u64,
    availability: &'static str,
    media: Option<&'a MediaStatus>,
    inspection: Option<&'a InspectionSnapshot>,
    capabilities: Vec<JblCapabilityBody>,
    controls: JblControlsBody,
}

#[derive(Serialize)]
struct JblCapabilityBody {
    id: &'static str,
    maturity: &'static str,
}

#[derive(Serialize)]
struct JblControlsBody {
    volume: JblBoundedControlBody,
    mute: JblSimpleControlBody,
    sources: JblTargetControlBody,
    eq: JblEqControlBody,
}

#[derive(Serialize)]
struct JblBoundedControlBody {
    enabled: bool,
    min: u8,
    max: u8,
}

#[derive(Serialize)]
struct JblSimpleControlBody {
    enabled: bool,
}

#[derive(Serialize)]
struct JblTargetControlBody {
    enabled: bool,
    targets: Vec<&'static str>,
}

#[derive(Serialize)]
struct JblEqControlBody {
    enabled: bool,
    targets: Vec<&'static str>,
    active: Option<&'static str>,
}

#[derive(Serialize)]
struct JblActionBody {
    action: &'static str,
    outcome: &'static str,
    failure: Option<&'static str>,
    revision: u64,
}

fn readiness_response() -> HttpResponse {
    json_response(200, &ReadinessBody { status: "ready" }, None)
}

const JBL_WEB_CAPABILITIES: &[&str] = &[
    "device_info",
    "media_status",
    "media_source",
    "volume_read",
    "mute_read",
    "eq_read",
    "personal_listening_read",
    "audio_sync_read",
    "source_list_read",
    "volume_set",
    "mute_set",
    "source_set",
    "eq_set",
];

fn jbl_status_response(snapshot: DirectSnapshot) -> HttpResponse {
    let revision = snapshot.revision;
    let known_volume = snapshot.media.playback.volume.is_some();
    let safe_volume = snapshot
        .media
        .playback
        .volume
        .is_some_and(|volume| volume <= 9);
    let known_mute = snapshot.media.playback.muted.is_some();
    let capabilities = snapshot
        .capabilities
        .iter()
        .filter(|capability| JBL_WEB_CAPABILITIES.contains(&capability.id))
        .map(|capability| JblCapabilityBody {
            id: capability.id,
            maturity: capability_maturity_name(capability.maturity),
        })
        .collect();
    let source_enabled = verified_capability(&snapshot.capabilities, "source_set");
    let source_targets = snapshot
        .source_targets
        .iter()
        .copied()
        .map(source_target_name)
        .collect::<Vec<_>>();
    let eq_enabled = verified_capability(&snapshot.capabilities, "eq_set")
        && safe_volume
        && snapshot.active_eq.is_some();
    let eq_targets = if eq_enabled {
        vec!["signature", "vocal", "energetic", "chill"]
    } else {
        Vec::new()
    };
    let body = JblStatusBody {
        schema_version: 1,
        revision,
        availability: "available",
        media: Some(&snapshot.media),
        inspection: Some(&snapshot.inspection),
        capabilities,
        controls: JblControlsBody {
            volume: JblBoundedControlBody {
                enabled: verified_capability(&snapshot.capabilities, "volume_set") && known_volume,
                min: 0,
                max: 9,
            },
            mute: JblSimpleControlBody {
                enabled: verified_capability(&snapshot.capabilities, "mute_set")
                    && known_mute
                    && safe_volume,
            },
            sources: JblTargetControlBody {
                enabled: source_enabled && safe_volume && !source_targets.is_empty(),
                targets: source_targets,
            },
            eq: JblEqControlBody {
                enabled: eq_enabled,
                targets: eq_targets,
                active: snapshot.active_eq.map(eq_target_name),
            },
        },
    };
    json_response(200, &body, Some(revision))
}

fn verified_capability(capabilities: &[Capability], id: &str) -> bool {
    capabilities.iter().any(|capability| {
        capability.id == id && capability.maturity == CapabilityMaturity::ImplementedVerifiedWrite
    })
}

fn jbl_action_response(action: JblWebAction, result: DirectActionResult) -> HttpResponse {
    let revision = result.revision;
    let body = JblActionBody {
        action: jbl_action_name(action),
        outcome: jbl_outcome_name(result.outcome),
        failure: result.failure.map(jbl_failure_name),
        revision,
    };
    json_response(200, &body, Some(revision))
}

fn jbl_unavailable_response() -> HttpResponse {
    json_response(
        503,
        &ErrorBody {
            error: "jbl_unavailable",
        },
        None,
    )
}

fn status_response(status: ControllerStatus) -> HttpResponse {
    let revision = status.revision();
    let body = StatusBody {
        backend: backend_name(status.backend()),
        pair_configuration: pair_configuration_name(status.pair_configuration()),
        members: status.members().each_ref().map(member_body),
        managed_state: managed_state_name(status.managed_state()),
        backend_health: status.backend_health().map(health_body),
        unresolved_action: status.has_unresolved_action(),
        consecutive_failures: status.consecutive_failures(),
        revision,
        last_action: status.last_action().map(last_action_body),
    };
    json_response(200, &body, Some(revision))
}

fn member_body(member: &PairMemberStatus) -> MemberBody {
    MemberBody {
        name: member_name(member.name()),
        verification: member_verification_name(member.verification()),
        channels: member
            .channels()
            .iter()
            .copied()
            .map(member_channel_name)
            .collect(),
    }
}

fn last_action_body(action: LastActionStatus) -> LastActionBody {
    LastActionBody {
        action: action_name(action.action()),
        outcome: outcome_name(action.outcome()),
        evidence: action.evidence().map(evidence_name),
        failure: action.failure().map(failure_name),
        revision: action.revision(),
        age_ms: action.age_ms(),
    }
}

fn member_name(value: PairMemberName) -> &'static str {
    match value {
        PairMemberName::JblAuthentics300 => "JBL Authentics 300",
        PairMemberName::AuraStudio5 => "Aura Studio 5",
    }
}

fn member_verification_name(value: PairMemberVerification) -> &'static str {
    match value {
        PairMemberVerification::Verified => "verified",
        PairMemberVerification::NotVerified => "not_verified",
        PairMemberVerification::Unavailable => "unavailable",
    }
}

fn member_channel_name(value: PairMemberChannel) -> &'static str {
    match value {
        PairMemberChannel::FrontLeft => "front_left",
        PairMemberChannel::FrontRight => "front_right",
        PairMemberChannel::Left => "left",
        PairMemberChannel::Right => "right",
        PairMemberChannel::Mono => "mono",
        PairMemberChannel::Stereo => "stereo",
        PairMemberChannel::Unknown => "unknown",
    }
}

fn action_response(result: ControllerActionResult) -> HttpResponse {
    let revision = result.revision();
    let body = ActionBody {
        action: action_name(result.action()),
        outcome: outcome_name(result.outcome()),
        managed_state: managed_state_name(result.managed_state()),
        evidence: result.evidence().map(evidence_name),
        failure: result.failure().map(failure_name),
        revision,
    };
    json_response(200, &body, Some(revision))
}

fn health_body(health: PairHealth) -> HealthBody {
    HealthBody {
        lifecycle: lifecycle_name(health.lifecycle()),
        level: health_level_name(health.level()),
        reported_error: health.has_reported_error(),
        aura_transport: aura_transport_name(health.aura_transport()),
        aura_acquisition_route: acquisition_route_name(health.aura_acquisition_route()),
    }
}

fn backend_name(value: PairBackendKind) -> &'static str {
    match value {
        PairBackendKind::LegacyV04WholePair => "legacy_v04_whole_pair",
        PairBackendKind::NativePair => "native_pair",
    }
}

fn pair_configuration_name(value: PairConfigurationState) -> &'static str {
    match value {
        PairConfigurationState::Ready => "ready",
        PairConfigurationState::NotReady => "not_ready",
        PairConfigurationState::Unavailable => "unavailable",
    }
}

fn managed_state_name(value: ManagedLiveState) -> &'static str {
    match value {
        ManagedLiveState::Unknown => "unknown",
        ManagedLiveState::Offline => "offline",
        ManagedLiveState::Ready => "ready",
        ManagedLiveState::Linking => "linking",
        ManagedLiveState::Linked => "linked",
        ManagedLiveState::Unlinking => "unlinking",
        ManagedLiveState::Recovering => "recovering",
        ManagedLiveState::Degraded => "degraded",
        ManagedLiveState::ShuttingDown => "shutting_down",
    }
}

fn lifecycle_name(value: PairLifecycle) -> &'static str {
    match value {
        PairLifecycle::Offline => "offline",
        PairLifecycle::Initializing => "initializing",
        PairLifecycle::Connecting => "connecting",
        PairLifecycle::Ready => "ready",
        PairLifecycle::Linking => "linking",
        PairLifecycle::Linked => "linked",
        PairLifecycle::Unlinking => "unlinking",
        PairLifecycle::Degraded => "degraded",
        PairLifecycle::Recovering => "recovering",
        PairLifecycle::ShuttingDown => "shutting_down",
        PairLifecycle::Failed => "failed",
    }
}

fn health_level_name(value: PairHealthLevel) -> &'static str {
    match value {
        PairHealthLevel::Healthy => "healthy",
        PairHealthLevel::Transitioning => "transitioning",
        PairHealthLevel::Degraded => "degraded",
        PairHealthLevel::Unavailable => "unavailable",
    }
}

fn aura_transport_name(value: AuraControlTransport) -> &'static str {
    match value {
        AuraControlTransport::Le => "le",
        AuraControlTransport::BrEdr => "br_edr",
        AuraControlTransport::Unresolved => "unresolved",
        AuraControlTransport::Unknown => "unknown",
    }
}

fn acquisition_route_name(value: AuraAcquisitionRoute) -> &'static str {
    match value {
        AuraAcquisitionRoute::StableDirect => "stable_direct",
        AuraAcquisitionRoute::A2dpWakeThenStable => "a2dp_wake_then_stable",
        AuraAcquisitionRoute::FreshLe => "fresh_le",
        AuraAcquisitionRoute::Unresolved => "unresolved",
    }
}

fn action_name(value: ControllerAction) -> &'static str {
    match value {
        ControllerAction::Start => "start",
        ControllerAction::Stop => "stop",
        ControllerAction::Shutdown => "shutdown",
        ControllerAction::RecoverStop => "recover_stop",
    }
}

fn source_target_name(value: AudioSourceTarget) -> &'static str {
    match value {
        AudioSourceTarget::Bluetooth => "bluetooth",
        AudioSourceTarget::AuxIn => "aux",
        AudioSourceTarget::UsbPlayback => "usb",
    }
}

fn eq_target_name(value: EqPresetTarget) -> &'static str {
    match value {
        EqPresetTarget::Signature => "signature",
        EqPresetTarget::Vocal => "vocal",
        EqPresetTarget::Energetic => "energetic",
        EqPresetTarget::Chill => "chill",
    }
}

fn capability_maturity_name(value: CapabilityMaturity) -> &'static str {
    match value {
        CapabilityMaturity::ImplementedReadOnly => "implemented_read_only",
        CapabilityMaturity::ImplementedVerifiedWrite => "implemented_verified_write",
        CapabilityMaturity::ProtocolPortedResearchOnly => "protocol_ported_research_only",
        CapabilityMaturity::SerializerOnly => "serializer_only",
        CapabilityMaturity::EvidenceRequired => "evidence_required",
        CapabilityMaturity::NotAdvertisedByExactProfile => "not_advertised_by_exact_profile",
        CapabilityMaturity::Forbidden => "forbidden",
    }
}

fn jbl_action_name(value: JblWebAction) -> &'static str {
    match value {
        JblWebAction::Volume => "volume_set",
        JblWebAction::Mute => "mute_set",
        JblWebAction::Source => "source_set",
        JblWebAction::EqPreset => "eq_preset_set",
    }
}

fn jbl_outcome_name(value: DirectActionOutcome) -> &'static str {
    match value {
        DirectActionOutcome::AlreadyAtTarget => "already_at_target",
        DirectActionOutcome::Applied => "applied",
        DirectActionOutcome::RejectedByDevice => "rejected_by_device",
        DirectActionOutcome::TargetObservedAfterUnknownWrite => {
            "target_observed_after_unknown_write"
        }
        DirectActionOutcome::PostconditionFailed => "postcondition_failed",
        DirectActionOutcome::RejectedBeforeSend => "rejected_before_send",
        DirectActionOutcome::OutcomeUnknown => "outcome_unknown",
    }
}

fn jbl_failure_name(value: DirectFailure) -> &'static str {
    match value {
        DirectFailure::Unavailable => "unavailable",
        DirectFailure::SafetyGate => "safety_gate",
        DirectFailure::UnsupportedTarget => "unsupported_target",
        DirectFailure::DeviceRejected => "device_rejected",
        DirectFailure::InvalidState => "invalid_state",
        DirectFailure::OutcomeUnknown => "outcome_unknown",
    }
}

fn outcome_name(value: ControllerActionOutcome) -> &'static str {
    match value {
        ControllerActionOutcome::Accepted => "accepted",
        ControllerActionOutcome::AcceptedUnconfirmed => "accepted_unconfirmed",
        ControllerActionOutcome::Idempotent => "idempotent",
        ControllerActionOutcome::RejectedBeforeSend => "rejected_before_send",
        ControllerActionOutcome::OutcomeUnknown => "outcome_unknown",
        ControllerActionOutcome::PostconditionFailed => "postcondition_failed",
    }
}

fn evidence_name(value: PairBackendEvidence) -> &'static str {
    match value {
        PairBackendEvidence::LocalSessionState => "local_session_state",
        PairBackendEvidence::LifecycleAcknowledgement => "lifecycle_acknowledgement",
        PairBackendEvidence::BroadcastAcknowledgementOnly => "broadcast_acknowledgement_only",
        PairBackendEvidence::BroadcastBusinessNotification => "broadcast_business_notification",
    }
}

fn failure_name(value: ControllerFailure) -> &'static str {
    match value {
        ControllerFailure::PairConfigurationUnavailable => "pair_configuration_unavailable",
        ControllerFailure::ExpectedPairNotConfigured => "expected_pair_not_configured",
        ControllerFailure::BackendRejectedBeforeSend => "backend_rejected_before_send",
        ControllerFailure::AuraInvalidConfiguration => "aura_invalid_configuration",
        ControllerFailure::AuraRuntimeUnavailable => "aura_runtime_unavailable",
        ControllerFailure::AuraAdapterUnavailable => "aura_adapter_unavailable",
        ControllerFailure::AuraDiscoveryUnavailable => "aura_discovery_unavailable",
        ControllerFailure::AuraVerifiedAdvertisementNotFound => {
            "aura_verified_advertisement_not_found"
        }
        ControllerFailure::AuraDeviceConnectionFailed => "aura_device_connection_failed",
        ControllerFailure::AuraWakeProfileConnectFailed => "wake_profile_connect_failed",
        ControllerFailure::AuraWakeFddfTimedOut => "wake_fddf_timed_out",
        ControllerFailure::AuraWakeFddfInvalid => "wake_fddf_invalid",
        ControllerFailure::AuraWakeFddfUnavailable => "wake_fddf_unavailable",
        ControllerFailure::AuraWakeProfileReleaseFailed => "wake_profile_release_failed",
        ControllerFailure::AuraGattProfileInvalid => "aura_gatt_profile_invalid",
        ControllerFailure::AuraNotificationSetupFailed => "aura_notification_setup_failed",
        ControllerFailure::AuraTransportNotReady => "aura_transport_not_ready",
        ControllerFailure::AuraNotificationQueueInvalid => "aura_notification_queue_invalid",
        ControllerFailure::AuraDisconnectFailed => "aura_disconnect_failed",
        ControllerFailure::AuraWriteUnknown => "aura_write_unknown",
        ControllerFailure::AuraAckTimeout => "aura_ack_timeout",
        ControllerFailure::AuraAckChannelClosed => "aura_ack_channel_closed",
        ControllerFailure::AuraUnexpectedAck => "aura_unexpected_ack",
        ControllerFailure::JblEnterOutcomeUnknown => "jbl_enter_outcome_unknown",
        ControllerFailure::JblExitOutcomeUnknown => "jbl_exit_outcome_unknown",
        ControllerFailure::JblBroadcastResultTimedOut => "jbl_broadcast_result_timed_out",
        ControllerFailure::JblBroadcastResultUnavailable => "jbl_broadcast_result_unavailable",
        ControllerFailure::JblBroadcastResultRejected => "jbl_broadcast_result_rejected",
        ControllerFailure::AuraStartOutcomeUnknown => "aura_start_outcome_unknown",
        ControllerFailure::BackendOutcomeUnknown => "backend_outcome_unknown",
        ControllerFailure::UnexpectedBackendLifecycle => "unexpected_backend_lifecycle",
        ControllerFailure::MembershipPostconditionFailed => "membership_postcondition_failed",
        ControllerFailure::UnresolvedPriorAction => "unresolved_prior_action",
        ControllerFailure::RecoveryNotAllowed => "recovery_not_allowed",
        ControllerFailure::JournalUnavailable => "journal_unavailable",
        ControllerFailure::JournalCommitFailed => "journal_commit_failed",
    }
}

fn json_response<T: Serialize>(status: u16, body: &T, revision: Option<u64>) -> HttpResponse {
    let body = serde_json::to_vec(body).expect("allowlisted Web response is serializable");
    let mut additional = Vec::new();
    if let Some(revision) = revision {
        additional.push(("ETag", format!("\"{revision}\"")));
    }
    response(
        status,
        "application/json; charset=utf-8",
        body,
        None,
        additional,
    )
}

fn error_response(error: RequestError) -> HttpResponse {
    let (status, name) = match error {
        RequestError::BadRequest | RequestError::InvalidJsonBody => (400, "bad_request"),
        RequestError::ForbiddenHost => (403, "host_rejected"),
        RequestError::ForbiddenOrigin => (403, "origin_rejected"),
        RequestError::CsrfRejected => (403, "csrf_rejected"),
        RequestError::NotFound => (404, "not_found"),
        RequestError::MethodNotAllowed => (405, "method_not_allowed"),
        RequestError::RevisionConflict => (409, "revision_conflict"),
        RequestError::LengthRequired => (411, "length_required"),
        RequestError::PayloadTooLarge => (413, "payload_too_large"),
        RequestError::UnsupportedMediaType => (415, "unsupported_media_type"),
        RequestError::PreconditionRequired => (428, "revision_required"),
        RequestError::HttpVersionNotSupported => (505, "http_version_not_supported"),
    };
    json_response(status, &ErrorBody { error: name }, None)
}

fn response<I, K, V>(
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    csp_nonce: Option<&str>,
    additional_headers: I,
) -> HttpResponse
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let csp = match csp_nonce {
        Some(nonce) => format!(
            "default-src 'self'; script-src 'nonce-{nonce}'; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
        ),
        None => "default-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'".to_string(),
    };
    let mut headers = vec![
        ("Content-Type".to_string(), content_type.to_string()),
        ("Content-Length".to_string(), body.len().to_string()),
        ("Connection".to_string(), "close".to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
        ("Pragma".to_string(), "no-cache".to_string()),
        ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
        ("Referrer-Policy".to_string(), "no-referrer".to_string()),
        ("Content-Security-Policy".to_string(), csp),
        ("X-Frame-Options".to_string(), "DENY".to_string()),
    ];
    headers.extend(
        additional_headers
            .into_iter()
            .map(|(name, value)| (name.into(), value.into())),
    );
    HttpResponse {
        status,
        headers,
        body,
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        428 => "Precondition Required",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}

fn is_tchar(byte: u8) -> bool {
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
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Compile-time embedded resource.  The only runtime substitution is the
/// validated 64-character hexadecimal CSP nonce.
const PAGE_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Play Together</title></head>
<body>
<main><h1>Play Together</h1>
<p role="status" aria-live="polite"><strong id="message">正在读取…</strong></p>
<dl><dt>本地管理状态</dt><dd id="managed">—</dd><dt>双成员配置</dt><dd id="pair">—</dd><dt>控制通道 / 本地生命周期</dt><dd id="health">—</dd><dt>Aura 获取路径</dt><dd id="route">—</dd><dt>状态版本</dt><dd id="revision">—</dd></dl>
<h2>成员</h2><ul><li>JBL Authentics 300：<span id="jbl">—</span></li><li>Aura Studio 5：<span id="aura">—</span></li></ul>
<h2>最近操作</h2><p id="last">本进程尚无操作</p>
<button id="start" type="button">启动</button><button id="stop" type="button">停止</button>
<section aria-labelledby="jbl-heading"><h1 id="jbl-heading">JBL Authentics 300（本地控制）</h1>
<p role="status" aria-live="polite"><strong id="jbl-message">正在读取…</strong></p>
<h2>媒体状态</h2><dl><dt>音源</dt><dd id="jbl-source">—</dd><dt>播放状态</dt><dd id="jbl-activity">—</dd><dt>音量</dt><dd id="jbl-volume">—</dd><dt>静音</dt><dd id="jbl-muted">—</dd></dl>
<h2>设备检查</h2><dl><dt>EQ 预设数</dt><dd id="jbl-eq-count">—</dd><dt>EQ 当前预设</dt><dd id="jbl-eq-active">—</dd><dt>EQ 数组长度</dt><dd id="jbl-eq-shape">—</dd><dt>Personal Listening</dt><dd id="jbl-personal">—</dd><dt>Audio Sync</dt><dd id="jbl-sync">—</dd><dt>能力</dt><dd id="jbl-capabilities">—</dd></dl>
<h2>安全控制</h2><p>切换音源可能解除静音；仅在音量≤9时执行。EQ 仅四个官方预设，不提供自定义设置。</p>
<button id="jbl-refresh" type="button">刷新 JBL 状态</button>
<label for="jbl-volume-input">音量 0–9</label><input id="jbl-volume-input" type="number" min="0" max="9" step="1"><button id="jbl-volume-apply" type="button" disabled>应用音量</button>
<button id="jbl-mute-on" type="button" disabled>静音</button><button id="jbl-mute-off" type="button" disabled>取消静音</button>
<div><strong>音源</strong><span id="jbl-source-controls"></span></div>
<div><strong>EQ 预设</strong><button type="button" data-eq="signature" disabled>Signature</button><button type="button" data-eq="vocal" disabled>Vocal</button><button type="button" data-eq="energetic" disabled>Energetic</button><button type="button" data-eq="chill" disabled>Chill</button></div>
</section></main>
<script nonce="__CSP_NONCE__">
"use strict";
let revision = null;
let jblMutationInFlight = false;
const byId=id=>document.getElementById(id);
const ackOnlyWarning="仅收到厂商控制ACK；琉璃是否出声未验证，linked/healthy不代表声学成功";
const weakWriteWarning="写入回复丢失；读回看见目标，但结果仍未知，请勿重试";
const browserWriteWarning="浏览器未收到完整写入结果；结果未知，请勿重试。控制保持禁用，请先刷新 JBL 状态";
function csrf(){const item=document.cookie.split(";").map(v=>v.trim()).find(v=>v.startsWith("jbl_aura_csrf="));return item?item.slice("jbl_aura_csrf=".length):"";}
function memberText(member){return member.verification+"；声道 "+member.channels.join(", ");}
function statusMessage(data){if(data.unresolved_action){return "存在未决操作，需要命令行恢复";}const last=data.last_action;if(last&&(last.outcome==="accepted_unconfirmed"||last.evidence==="broadcast_acknowledgement_only")){return ackOnlyWarning;}if(last&&last.outcome==="idempotent"){return "本次操作未写设备。";}return "状态已更新";}
function updateRevision(value){revision=value;}
function render(data){updateRevision(data.revision);byId("managed").textContent=data.managed_state;byId("pair").textContent=data.pair_configuration;byId("health").textContent=data.backend_health?data.backend_health.level+" / "+data.backend_health.lifecycle:"unavailable";byId("route").textContent=data.backend_health?data.backend_health.aura_acquisition_route:"unresolved";byId("revision").textContent=String(data.revision);const members=new Map(data.members.map(member=>[member.name,member]));byId("jbl").textContent=memberText(members.get("JBL Authentics 300"));byId("aura").textContent=memberText(members.get("Aura Studio 5"));const last=data.last_action;byId("last").textContent=last?last.action+" / "+last.outcome+" / evidence="+(last.evidence??"none")+" / failure="+(last.failure??"none")+" / revision="+last.revision+" / age_ms="+last.age_ms:"本进程尚无操作";byId("message").textContent=statusMessage(data);}
function sourceLabel(value){return value==="bluetooth"?"蓝牙":value==="aux"?"AUX":value==="usb"?"USB":"未知";}
function disableJbl(){byId("jbl-volume-apply").disabled=true;byId("jbl-mute-on").disabled=true;byId("jbl-mute-off").disabled=true;byId("jbl-source-controls").replaceChildren();for(const button of document.querySelectorAll("[data-eq]")){button.disabled=true;}}
function renderJbl(data){updateRevision(data.revision);const available=data.availability==="available"&&data.media&&data.inspection;const interactive=available&&!jblMutationInFlight;byId("jbl-message").textContent=available?"状态已更新":"JBL 状态暂不可用，控制已禁用";byId("jbl-source").textContent=available?data.media.source:"unknown";byId("jbl-activity").textContent=available?data.media.playback.state:"unknown";byId("jbl-volume").textContent=available&&data.media.playback.volume!==null?String(data.media.playback.volume):"unknown";byId("jbl-muted").textContent=available&&data.media.playback.muted!==null?(data.media.playback.muted?"是":"否"):"unknown";byId("jbl-volume-input").value=available&&data.media.playback.volume!==null?String(data.media.playback.volume):"";byId("jbl-eq-count").textContent=available?String(data.inspection.eq.preset_count):"unknown";byId("jbl-eq-active").textContent=data.controls.eq.active??"unknown";byId("jbl-eq-shape").textContent=available?data.inspection.eq.fs_count+"/"+data.inspection.eq.gain_count+"/"+data.inspection.eq.q_count+"/"+data.inspection.eq.type_count:"unknown";byId("jbl-personal").textContent=available?data.inspection.personal_listening:"unknown";byId("jbl-sync").textContent=available?String(data.inspection.audio_sync):"unknown";byId("jbl-capabilities").textContent=data.capabilities.map(item=>item.id+":"+item.maturity).join(", ");byId("jbl-volume-apply").disabled=!interactive||!data.controls.volume.enabled;byId("jbl-mute-on").disabled=!interactive||!data.controls.mute.enabled;byId("jbl-mute-off").disabled=!interactive||!data.controls.mute.enabled;const sources=byId("jbl-source-controls");sources.replaceChildren();for(const target of data.controls.sources.targets){const button=document.createElement("button");button.type="button";button.textContent=sourceLabel(target);button.disabled=!interactive||!data.controls.sources.enabled;button.addEventListener("click",()=>mutateJbl("source",'{"target":"'+target+'","confirm":"source-set"}'));sources.append(button);}for(const button of document.querySelectorAll("[data-eq]")){button.disabled=!interactive||!data.controls.eq.enabled||!data.controls.eq.targets.includes(button.dataset.eq);}}
function jblResultMessage(result){if(result.outcome==="applied"){return "已执行并读回确认";}if(result.outcome==="already_at_target"){return "已是目标状态，本次未写设备";}if(result.outcome==="target_observed_after_unknown_write"||result.outcome==="outcome_unknown"){return weakWriteWarning;}if(result.outcome==="rejected_by_device"){return "设备拒绝了请求";}if(result.outcome==="postcondition_failed"){return "设备读回与目标不一致";}return "请求在写入前被拒绝";}
async function refresh(){try{const pair=await fetch("/api/status",{cache:"no-store"});if(!pair.ok){throw new Error("pair unavailable");}render(await pair.json());}catch(_error){disableJbl();byId("message").textContent="状态读取失败";byId("jbl-message").textContent="整体状态读取失败，JBL 控制已禁用";return false;}try{const jbl=await fetch("/api/jbl/status",{cache:"no-store"});if(!jbl.ok){throw new Error("jbl unavailable");}renderJbl(await jbl.json());return true;}catch(_error){disableJbl();byId("jbl-message").textContent="JBL 状态读取失败，控制已禁用";return false;}}
async function mutate(action){if(revision===null){await refresh();}const response=await fetch("/api/"+action,{method:"POST",headers:{"Content-Type":"application/json","X-CSRF-Token":csrf(),"If-Match":"\""+revision+"\""},body:"{}"});if(response.status===409){await refresh();return;}await response.json();await refresh();}
async function mutateJbl(action,body){if(jblMutationInFlight){return;}jblMutationInFlight=true;disableJbl();byId("jbl-refresh").disabled=true;try{if(revision===null&&!(await refresh())){return;}const response=await fetch("/api/jbl/"+action,{method:"POST",headers:{"Content-Type":"application/json","X-CSRF-Token":csrf(),"If-Match":"\""+revision+"\""},body});if(response.status===409){jblMutationInFlight=false;await refresh();return;}const result=await response.json();const message=response.ok?jblResultMessage(result):"请求失败，未自动重试";jblMutationInFlight=false;if(await refresh()){byId("jbl-message").textContent=message;}else{disableJbl();byId("jbl-message").textContent="请求已返回，但状态刷新失败；控制已禁用";}}catch(_error){jblMutationInFlight=false;disableJbl();byId("jbl-message").textContent=browserWriteWarning;}finally{jblMutationInFlight=false;byId("jbl-refresh").disabled=false;}}
document.getElementById("start").addEventListener("click",()=>mutate("start"));
document.getElementById("stop").addEventListener("click",()=>mutate("stop"));
document.getElementById("jbl-volume-apply").addEventListener("click",()=>{const value=Number(byId("jbl-volume-input").value);if(Number.isInteger(value)&&value>=0&&value<=9){mutateJbl("volume",'{"value":'+value+',"confirm":"volume-set"}');}});
document.getElementById("jbl-mute-on").addEventListener("click",()=>mutateJbl("mute",'{"state":"on","confirm":"mute-set"}'));
document.getElementById("jbl-mute-off").addEventListener("click",()=>mutateJbl("mute",'{"state":"off","confirm":"mute-set"}'));
for(const button of document.querySelectorAll("[data-eq]")){button.addEventListener("click",()=>mutateJbl("eq-preset",'{"target":"'+button.dataset.eq+'","confirm":"eq-preset-set"}'));}
document.getElementById("jbl-refresh").addEventListener("click",async()=>{if(jblMutationInFlight){return;}byId("jbl-refresh").disabled=true;await refresh();byId("jbl-refresh").disabled=false;});
refresh().catch(()=>{byId("message").textContent="状态读取失败";});
</script>
</body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{
        AuraControlTransport, PairActionReceipt, PairActionResult, PairBackend, PairBackendError,
        PairBackendEvidence, PairBackendKind, PairHealth, PairLifecycle,
    };
    use crate::controller::{
        PairConfigurationObservation, PairConfigurationProbe, PairController, PairProbeError,
    };
    use crate::journal::MemoryJournal;
    use crate::media::{MediaSource, PlaybackStatus, TransportState, TransportStatus};
    use crate::{
        authentics_300_capabilities, AudioSourceSummary, EqSummary, FeatureSupportSummary,
        MediaSourceActivity, MediaSourceActivitySummary, PersonalListeningState,
    };

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
        mutation_calls: usize,
        mutations: usize,
    }

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
            self.mutation_calls += 1;
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

    fn app() -> WebApp<Actor> {
        WebApp::new(
            Actor {
                controller: PairController::new(
                    Backend {
                        health: PairLifecycle::Ready,
                    },
                    Probe,
                ),
                status_calls: 0,
                mutation_calls: 0,
                mutations: 0,
            },
            WebSecurity::loopback([0x5a; 32]).expect("security"),
        )
    }

    struct DirectActor {
        pair: Actor,
        snapshot: DirectSnapshot,
        direct_calls: usize,
        direct_mutations: Vec<DirectMutation>,
        private_marker: &'static str,
    }

    impl WebActor for DirectActor {
        fn status(&mut self) -> ControllerStatus {
            self.pair.status()
        }

        fn mutate_if_revision(
            &mut self,
            expected_revision: u64,
            mutation: WebMutation,
        ) -> Result<ControllerActionResult, RevisionConflict> {
            self.pair.mutate_if_revision(expected_revision, mutation)
        }

        fn direct_snapshot(&mut self) -> Result<DirectSnapshot, DirectFailure> {
            Ok(self.snapshot.clone())
        }

        fn mutate_direct_if_revision(
            &mut self,
            expected_revision: u64,
            mutation: DirectMutation,
        ) -> Result<DirectActionResult, RevisionConflict> {
            if expected_revision != self.snapshot.revision {
                return Err(RevisionConflict);
            }
            self.direct_calls += 1;
            self.direct_mutations.push(mutation);
            self.snapshot.revision += 1;
            Ok(DirectActionResult {
                outcome: DirectActionOutcome::Applied,
                observation: None,
                failure: None,
                revision: self.snapshot.revision,
            })
        }
    }

    fn direct_snapshot() -> DirectSnapshot {
        DirectSnapshot {
            media: MediaStatus {
                playback: PlaybackStatus {
                    state: TransportState::Stopped,
                    transport_status: TransportStatus::Ok,
                    volume: Some(9),
                    muted: Some(false),
                },
                source: MediaSource::Bluetooth,
            },
            inspection: InspectionSnapshot {
                feature_support: FeatureSupportSummary {
                    known: Vec::new(),
                    unknown_key_count: 1,
                },
                eq: EqSummary {
                    preset_count: 5,
                    active_present: true,
                    fs_count: 3,
                    gain_count: 3,
                    q_count: 3,
                    type_count: 3,
                },
                audio_sources: AudioSourceSummary {
                    active: MediaSource::Bluetooth,
                    support_sources: vec![MediaSource::Bluetooth, MediaSource::AuxIn],
                },
                personal_listening: PersonalListeningState::Off,
                audio_sync: 0,
                media_source_activity: MediaSourceActivitySummary {
                    source: MediaSource::Bluetooth,
                    activity: MediaSourceActivity::Stopped,
                },
            },
            capabilities: authentics_300_capabilities().to_vec(),
            source_targets: vec![AudioSourceTarget::Bluetooth, AudioSourceTarget::AuxIn],
            active_eq: Some(EqPresetTarget::Signature),
            revision: 0,
        }
    }

    fn direct_app() -> WebApp<DirectActor> {
        let pair = app().into_actor();
        WebApp::new(
            DirectActor {
                pair,
                snapshot: direct_snapshot(),
                direct_calls: 0,
                direct_mutations: Vec::new(),
                private_marker: "private-ip-uuid-cert-path-marker",
            },
            WebSecurity::loopback([0x5a; 32]).expect("security"),
        )
    }

    fn token() -> String {
        "5a".repeat(32)
    }

    fn get(path: &str, host: &str) -> Vec<u8> {
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes()
    }

    fn post(path: &str, revision: u64, csrf_header: &str, csrf_cookie: &str) -> Vec<u8> {
        let body = "{}";
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:8096\r\nOrigin: http://127.0.0.1:8096\r\nContent-Type: application/json\r\nContent-Length: {}\r\nIf-Match: \"{revision}\"\r\nX-CSRF-Token: {csrf_header}\r\nCookie: {CSRF_COOKIE}={csrf_cookie}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn post_json(
        path: &str,
        revision: u64,
        csrf_header: &str,
        csrf_cookie: &str,
        body: &str,
    ) -> Vec<u8> {
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:8096\r\nOrigin: http://127.0.0.1:8096\r\nContent-Type: application/json\r\nContent-Length: {}\r\nIf-Match: \"{revision}\"\r\nX-CSRF-Token: {csrf_header}\r\nCookie: {CSRF_COOKIE}={csrf_cookie}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn json(response: &HttpResponse) -> serde_json::Value {
        serde_json::from_slice(response.body()).expect("JSON response")
    }

    #[test]
    fn security_configuration_has_no_non_loopback_escape_hatch() {
        assert_eq!(
            WebSecurity::new("192.0.2.1:8096".parse().unwrap(), [1; 32]).err(),
            Some(WebConfigError::NonLoopbackBind)
        );
        assert_eq!(
            WebSecurity::new("127.0.0.2:8096".parse().unwrap(), [1; 32]).err(),
            Some(WebConfigError::NonLoopbackBind)
        );
        assert_eq!(
            WebSecurity::new("127.0.0.1:0".parse().unwrap(), [1; 32]).err(),
            Some(WebConfigError::InvalidPort)
        );
        assert_eq!(
            WebSecurity::loopback([0; 32]).err(),
            Some(WebConfigError::InvalidCsrfToken)
        );
    }

    #[test]
    fn page_is_embedded_and_every_response_has_security_headers() {
        let response = app().handle(&get("/", "localhost:8096"));
        assert_eq!(response.status(), 200);
        let body = std::str::from_utf8(response.body()).unwrap();
        assert!(body.contains("Play Together"));
        assert!(body.contains("JBL Authentics 300"));
        assert!(body.contains("Aura Studio 5"));
        assert!(body.contains("最近操作"));
        assert!(body.contains("本地管理状态"));
        assert!(body.contains("控制通道 / 本地生命周期"));
        assert!(body.contains("<strong id=\"message\">"));
        assert!(!body.contains("<dt>运行状态</dt>"));
        assert!(!body.contains("<dt>后端健康</dt>"));
        assert!(!body.contains("JSON.stringify"));
        assert!(!body.contains("recover-stop"));
        assert!(body.contains("JBL Authentics 300（本地控制）"));
        assert!(body.contains("id=\"jbl-refresh\""));
        assert!(body.contains("jbl-volume-apply"));
        assert!(body.contains("data-eq=\"signature\""));
        assert!(!body.contains("/api/jbl/playback"));
        assert!(!body.contains("product-setting"));
        assert!(!body.contains("id=\"play\""));
        assert!(!body.contains("innerHTML"));
        assert!(body.contains("function disableJbl()"));
        assert!(body.contains("catch(_error){disableJbl();"));
        assert!(body.contains("if(jblMutationInFlight){return;}jblMutationInFlight=true"));
        assert!(body.contains("finally{jblMutationInFlight=false;"));
        assert!(body.contains("function updateRevision(value){revision=value;}"));
        assert!(!body.contains("Math.max(revision"));
        assert!(body.contains("浏览器未收到完整写入结果；结果未知，请勿重试"));
        assert!(body.contains("控制保持禁用，请先刷新 JBL 状态"));
        assert!(body.contains("if(await refresh()){byId(\"jbl-message\").textContent=message;}"));
        assert!(body.contains("结果仍未知，请勿重试"));
        assert!(!body.contains("__CSP_NONCE__"));
        assert!(response
            .header("Content-Security-Policy")
            .unwrap()
            .contains("default-src 'self'"));
        assert!(response
            .header("Content-Security-Policy")
            .unwrap()
            .contains("frame-ancestors 'none'"));
        assert_eq!(response.header("X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(response.header("Cache-Control"), Some("no-store"));
        assert_eq!(response.header("Referrer-Policy"), Some("no-referrer"));
        assert!(response
            .header("Set-Cookie")
            .unwrap()
            .contains("SameSite=Strict"));
    }

    #[test]
    fn default_actor_jbl_status_is_closed_unavailable() {
        let mut app = app();
        let response = app.handle(&get("/api/jbl/status", "127.0.0.1:8096"));
        assert_eq!(response.status(), 503);
        assert_eq!(
            json(&response),
            serde_json::json!({"error":"jbl_unavailable"})
        );
    }

    #[test]
    fn jbl_status_contains_only_sanitized_cards_and_filtered_capabilities() {
        let mut app = direct_app();
        let response = app.handle(&get("/api/jbl/status", "127.0.0.1:8096"));
        assert_eq!(response.status(), 200);
        let body = json(&response);
        assert_eq!(body["schema_version"], 1);
        assert_eq!(body["availability"], "available");
        assert_eq!(body["media"]["source"], "bluetooth");
        assert_eq!(body["inspection"]["eq"]["preset_count"], 5);
        assert_eq!(body["controls"]["volume"]["max"], 9);
        assert_eq!(
            body["controls"]["sources"]["targets"],
            serde_json::json!(["bluetooth", "aux"])
        );
        assert_eq!(body["controls"]["eq"]["active"], "signature");
        let encoded = std::str::from_utf8(response.body()).unwrap();
        let actor = app.into_actor();
        assert!(!encoded.contains(actor.private_marker));
        assert!(!encoded.contains("playback_mutation"));
        assert!(!encoded.contains("product_settings_read"));
    }

    #[test]
    fn unsafe_or_unknown_snapshot_disables_state_changing_controls() {
        let mut app = direct_app();
        app.actor.snapshot.media.playback.volume = Some(10);
        app.actor.snapshot.active_eq = None;
        let response = app.handle(&get("/api/jbl/status", "127.0.0.1:8096"));
        let body = json(&response);
        // Volume remains available to lower an existing loud value into the
        // 0..9 safe range; every action that can reveal or route audio stops.
        assert_eq!(body["controls"]["volume"]["enabled"], true);
        assert_eq!(body["controls"]["mute"]["enabled"], false);
        assert_eq!(body["controls"]["sources"]["enabled"], false);
        assert_eq!(body["controls"]["eq"]["enabled"], false);

        let mut app = direct_app();
        app.actor.snapshot.media.playback.volume = None;
        let response = app.handle(&get("/api/jbl/status", "127.0.0.1:8096"));
        assert_eq!(json(&response)["controls"]["volume"]["enabled"], false);
    }

    #[test]
    fn four_jbl_routes_require_exact_confirmed_bodies_and_revision() {
        let csrf = token();
        for (path, body, action, expected_mutation) in [
            (
                "/api/jbl/volume",
                r#"{"value":9,"confirm":"volume-set"}"#,
                "volume_set",
                DirectMutation::Volume(9),
            ),
            (
                "/api/jbl/mute",
                r#"{"state":"on","confirm":"mute-set"}"#,
                "mute_set",
                DirectMutation::Mute(MuteTarget::On),
            ),
            (
                "/api/jbl/source",
                r#"{"target":"aux","confirm":"source-set"}"#,
                "source_set",
                DirectMutation::Source(AudioSourceTarget::AuxIn),
            ),
            (
                "/api/jbl/eq-preset",
                r#"{"target":"vocal","confirm":"eq-preset-set"}"#,
                "eq_preset_set",
                DirectMutation::EqPreset(EqPresetTarget::Vocal),
            ),
        ] {
            let mut app = direct_app();
            let response = app.handle(&post_json(path, 0, &csrf, &csrf, body));
            assert_eq!(response.status(), 200);
            assert_eq!(json(&response)["action"], action);
            assert_eq!(json(&response)["outcome"], "applied");
            assert_eq!(json(&response)["revision"], 1);
            let actor = app.into_actor();
            assert_eq!(actor.direct_calls, 1);
            assert_eq!(actor.direct_mutations, vec![expected_mutation]);
        }
    }

    #[test]
    fn invalid_jbl_bodies_and_stale_revision_never_reach_actor_mutation() {
        let csrf = token();
        for (path, body) in [
            ("/api/jbl/volume", r#"{"value":10,"confirm":"volume-set"}"#),
            (
                "/api/jbl/volume",
                r#"{"value":9,"confirm":"volume-set","ip":"private"}"#,
            ),
            (
                "/api/jbl/volume",
                r#"{"value":9,"value":8,"confirm":"volume-set"}"#,
            ),
            (
                "/api/jbl/mute",
                r#"{"state":"on","confirm":"mute-set","confirm":"mute-set"}"#,
            ),
            (
                "/api/jbl/mute",
                r#"{"state":"toggle","confirm":"mute-set"}"#,
            ),
            (
                "/api/jbl/source",
                r#"{"target":"PRIVATE","confirm":"source-set"}"#,
            ),
            (
                "/api/jbl/eq-preset",
                r#"{"target":"custom","confirm":"eq-preset-set"}"#,
            ),
            (
                "/api/jbl/source",
                r#"{"command":"raw","confirm":"source-set"}"#,
            ),
        ] {
            let mut app = direct_app();
            let response = app.handle(&post_json(path, 0, &csrf, &csrf, body));
            assert_eq!(response.status(), 400);
            assert_eq!(app.into_actor().direct_calls, 0);
        }
        let mut app = direct_app();
        let response = app.handle(&post_json(
            "/api/jbl/mute",
            4,
            &csrf,
            &csrf,
            r#"{"state":"off","confirm":"mute-set"}"#,
        ));
        assert_eq!(response.status(), 409);
        assert_eq!(app.into_actor().direct_calls, 0);
    }

    #[test]
    fn direct_routes_reject_origin_csrf_and_missing_or_weak_revision_without_calls() {
        let csrf = token();
        let body = r#"{"state":"on","confirm":"mute-set"}"#;
        let base = String::from_utf8(post_json("/api/jbl/mute", 0, &csrf, &csrf, body)).unwrap();
        let cases = [
            (
                base.replace(
                    "Origin: http://127.0.0.1:8096",
                    "Origin: http://localhost:8096",
                ),
                403,
            ),
            (
                base.replace(&format!("X-CSRF-Token: {csrf}"), "X-CSRF-Token: wrong"),
                403,
            ),
            (base.replace("If-Match: \"0\"\r\n", ""), 428),
            (base.replace("If-Match: \"0\"", "If-Match: W/\"0\""), 428),
        ];
        for (request, expected_status) in cases {
            let mut app = direct_app();
            assert_eq!(app.handle(request.as_bytes()).status(), expected_status);
            assert_eq!(app.into_actor().direct_calls, 0);
        }
    }

    #[test]
    fn page_explicitly_distinguishes_ack_only_idempotence_and_acoustic_success() {
        let body = PAGE_HTML;
        assert!(
            body.contains("仅收到厂商控制ACK；琉璃是否出声未验证，linked/healthy不代表声学成功")
        );
        assert!(body.contains("last.outcome===\"accepted_unconfirmed\""));
        assert!(body.contains("last.evidence===\"broadcast_acknowledgement_only\""));
        assert!(body.contains("last.outcome===\"idempotent\""));
        assert!(body.contains("本次操作未写设备。"));
        assert!(!body.contains("琉璃已出声"));
        assert!(!body.contains("声学成功已验证"));
    }

    #[test]
    fn status_json_is_an_allowlisted_projection_with_revision_etag() {
        let response = app().handle(&get("/api/status", "[::1]:8096"));
        assert_eq!(response.status(), 200);
        let body = json(&response);
        assert_eq!(body["backend"], "legacy_v04_whole_pair");
        assert_eq!(body["pair_configuration"], "ready");
        assert_eq!(body["members"][0]["name"], "JBL Authentics 300");
        assert_eq!(body["members"][0]["verification"], "verified");
        assert_eq!(body["members"][0]["channels"][0], "unknown");
        assert_eq!(body["members"][1]["name"], "Aura Studio 5");
        assert!(body["last_action"].is_null());
        assert_eq!(body["backend_health"]["aura_transport"], "le");
        assert_eq!(
            body["backend_health"]["aura_acquisition_route"],
            "unresolved"
        );
        assert_eq!(body["revision"], 0);
        assert_eq!(response.header("ETag"), Some("\"0\""));
        assert_eq!(body.as_object().unwrap().len(), 9);
    }

    #[test]
    fn acquisition_route_labels_are_closed_and_fixed() {
        assert_eq!(
            acquisition_route_name(AuraAcquisitionRoute::StableDirect),
            "stable_direct"
        );
        assert_eq!(
            acquisition_route_name(AuraAcquisitionRoute::A2dpWakeThenStable),
            "a2dp_wake_then_stable"
        );
        assert_eq!(
            acquisition_route_name(AuraAcquisitionRoute::FreshLe),
            "fresh_le"
        );
        assert_eq!(
            acquisition_route_name(AuraAcquisitionRoute::Unresolved),
            "unresolved"
        );
    }

    #[test]
    fn diagnostic_failure_labels_are_fixed_and_non_identifying() {
        for (failure, expected) in [
            (
                ControllerFailure::AuraInvalidConfiguration,
                "aura_invalid_configuration",
            ),
            (
                ControllerFailure::AuraRuntimeUnavailable,
                "aura_runtime_unavailable",
            ),
            (
                ControllerFailure::AuraTransportNotReady,
                "aura_transport_not_ready",
            ),
            (
                ControllerFailure::AuraNotificationQueueInvalid,
                "aura_notification_queue_invalid",
            ),
            (
                ControllerFailure::AuraDisconnectFailed,
                "aura_disconnect_failed",
            ),
            (
                ControllerFailure::AuraWakeProfileConnectFailed,
                "wake_profile_connect_failed",
            ),
            (
                ControllerFailure::AuraWakeFddfTimedOut,
                "wake_fddf_timed_out",
            ),
            (ControllerFailure::AuraWakeFddfInvalid, "wake_fddf_invalid"),
            (
                ControllerFailure::AuraWakeFddfUnavailable,
                "wake_fddf_unavailable",
            ),
            (
                ControllerFailure::AuraWakeProfileReleaseFailed,
                "wake_profile_release_failed",
            ),
            (ControllerFailure::AuraWriteUnknown, "aura_write_unknown"),
            (ControllerFailure::AuraAckTimeout, "aura_ack_timeout"),
            (
                ControllerFailure::AuraAckChannelClosed,
                "aura_ack_channel_closed",
            ),
            (ControllerFailure::AuraUnexpectedAck, "aura_unexpected_ack"),
            (
                ControllerFailure::JblEnterOutcomeUnknown,
                "jbl_enter_outcome_unknown",
            ),
            (
                ControllerFailure::JblExitOutcomeUnknown,
                "jbl_exit_outcome_unknown",
            ),
            (
                ControllerFailure::JblBroadcastResultTimedOut,
                "jbl_broadcast_result_timed_out",
            ),
            (
                ControllerFailure::JblBroadcastResultUnavailable,
                "jbl_broadcast_result_unavailable",
            ),
            (
                ControllerFailure::JblBroadcastResultRejected,
                "jbl_broadcast_result_rejected",
            ),
            (
                ControllerFailure::AuraStartOutcomeUnknown,
                "aura_start_outcome_unknown",
            ),
        ] {
            assert_eq!(failure_name(failure), expected);
            assert!(expected
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
        }
    }

    #[test]
    fn valid_start_and_stop_only_cross_the_actor_trait() {
        let csrf = token();
        let mut app = app();
        let start = app.handle(&post("/api/start", 0, &csrf, &csrf));
        assert_eq!(start.status(), 200);
        assert_eq!(json(&start)["action"], "start");
        assert_eq!(json(&start)["outcome"], "accepted");
        assert_eq!(json(&start)["evidence"], "lifecycle_acknowledgement");
        assert_eq!(json(&start)["revision"], 2);
        let stop = app.handle(&post("/api/stop", 2, &csrf, &csrf));
        assert_eq!(stop.status(), 200);
        assert_eq!(json(&stop)["action"], "stop");
        assert_eq!(app.into_actor().mutations, 2);
    }

    #[test]
    fn unconfirmed_outcome_and_evidence_have_fixed_non_misleading_labels() {
        assert_eq!(
            outcome_name(ControllerActionOutcome::AcceptedUnconfirmed),
            "accepted_unconfirmed"
        );
        assert_eq!(
            evidence_name(PairBackendEvidence::BroadcastAcknowledgementOnly),
            "broadcast_acknowledgement_only"
        );
        assert_eq!(
            evidence_name(PairBackendEvidence::BroadcastBusinessNotification),
            "broadcast_business_notification"
        );
    }

    #[test]
    fn stale_revision_returns_conflict_without_mutation() {
        let csrf = token();
        let mut app = app();
        let response = app.handle(&post("/api/start", 1, &csrf, &csrf));
        assert_eq!(response.status(), 409);
        assert_eq!(json(&response)["error"], "revision_conflict");
        let actor = app.into_actor();
        assert_eq!(actor.status_calls, 0);
        assert_eq!(actor.mutation_calls, 1);
        assert_eq!(actor.mutations, 0);
    }

    #[test]
    fn healthz_is_fixed_and_never_calls_the_actor() {
        let mut app = app();
        let response = app.handle(&get("/healthz", "127.0.0.1:8096"));
        assert_eq!(response.status(), 200);
        assert_eq!(json(&response), serde_json::json!({ "status": "ready" }));
        let actor = app.into_actor();
        assert_eq!(actor.status_calls, 0);
        assert_eq!(actor.mutation_calls, 0);
        assert_eq!(actor.mutations, 0);
    }

    #[test]
    fn internal_recovery_requires_exact_confirmation_and_is_not_retried() {
        let csrf = token();
        let confirmed = String::from_utf8(post("/internal/recover-stop", 0, &csrf, &csrf))
            .unwrap()
            .replace("Content-Length: 2", "Content-Length: 26")
            .replace("\r\n\r\n{}", "\r\n\r\n{\"confirm\":\"recover-stop\"}");
        let mut confirmed_app = app();
        let response = confirmed_app.handle(confirmed.as_bytes());
        assert_eq!(response.status(), 200);
        assert_eq!(json(&response)["action"], "recover_stop");
        assert_eq!(json(&response)["failure"], "recovery_not_allowed");
        let actor = confirmed_app.into_actor();
        assert_eq!(actor.mutation_calls, 1);
        assert_eq!(actor.mutations, 1);

        for body in [
            "{}",
            "{\"confirm\":true}",
            "{\"confirm\":\"recover-stop\",\"extra\":1}",
            "{\"confirm\":\"stop\"}",
        ] {
            let base = String::from_utf8(post("/internal/recover-stop", 0, &csrf, &csrf)).unwrap();
            let request = base
                .replace(
                    "Content-Length: 2",
                    &format!("Content-Length: {}", body.len()),
                )
                .replace("\r\n\r\n{}", &format!("\r\n\r\n{body}"));
            let mut app = app();
            assert_eq!(app.handle(request.as_bytes()).status(), 400);
            let actor = app.into_actor();
            assert_eq!(actor.mutation_calls, 0);
            assert_eq!(actor.mutations, 0);
        }

        let stale = String::from_utf8(post("/internal/recover-stop", 1, &csrf, &csrf))
            .unwrap()
            .replace("Content-Length: 2", "Content-Length: 26")
            .replace("\r\n\r\n{}", "\r\n\r\n{\"confirm\":\"recover-stop\"}");
        let mut app = app();
        assert_eq!(app.handle(stale.as_bytes()).status(), 409);
        let actor = app.into_actor();
        assert_eq!(actor.mutation_calls, 1);
        assert_eq!(actor.mutations, 0);
    }

    #[test]
    fn host_is_restricted_to_exact_loopback_authorities_and_configured_port() {
        for rejected in [
            "example.test",
            "127.0.0.2:8096",
            "localhost.example:8096",
            "localhost:8080",
            "[::ffff:127.0.0.1]:8096",
            "localhost.:8096",
        ] {
            assert_eq!(app().handle(&get("/api/status", rejected)).status(), 403);
        }
        for accepted in ["localhost", "LOCALHOST:8096", "127.0.0.1", "[::1]:8096"] {
            assert_eq!(app().handle(&get("/api/status", accepted)).status(), 200);
        }
    }

    #[test]
    fn post_requires_same_origin() {
        let csrf = token();
        let mut request = post("/api/start", 0, &csrf, &csrf);
        let text = String::from_utf8(request).unwrap().replace(
            "Origin: http://127.0.0.1:8096",
            "Origin: http://localhost:8096",
        );
        request = text.into_bytes();
        assert_eq!(app().handle(&request).status(), 403);

        let missing = String::from_utf8(post("/api/start", 0, &csrf, &csrf))
            .unwrap()
            .replace("Origin: http://127.0.0.1:8096\r\n", "");
        assert_eq!(app().handle(missing.as_bytes()).status(), 403);
    }

    #[test]
    fn csrf_requires_matching_header_cookie_and_server_token() {
        let csrf = token();
        assert_eq!(
            app()
                .handle(&post("/api/start", 0, "wrong", &csrf))
                .status(),
            403
        );
        assert_eq!(
            app()
                .handle(&post("/api/start", 0, &csrf, "wrong"))
                .status(),
            403
        );
        let duplicate_cookie = String::from_utf8(post("/api/start", 0, &csrf, &csrf))
            .unwrap()
            .replace(
                &format!("Cookie: {CSRF_COOKIE}={csrf}"),
                &format!("Cookie: {CSRF_COOKIE}={csrf}; {CSRF_COOKIE}={csrf}"),
            );
        assert_eq!(app().handle(duplicate_cookie.as_bytes()).status(), 403);
    }

    #[test]
    fn request_smuggling_shapes_are_rejected() {
        let cases = [
            b"POST /api/start HTTP/1.1\r\nHost: localhost:8096\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"POST /api/start HTTP/1.1\r\nHost: localhost:8096\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".as_slice(),
            b"POST /api/start HTTP/1.1\r\nHost: localhost:8096\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n".as_slice(),
            b"GET /api/status HTTP/1.1\r\nHost: localhost:8096\r\n folded: yes\r\n\r\n".as_slice(),
            b"GET http://localhost:8096/api/status HTTP/1.1\r\nHost: localhost:8096\r\n\r\n".as_slice(),
            b"POST /api/start HTTP/1.1\r\nHost: localhost:8096\r\nContent-Length: 0\r\n\r\nGET / HTTP/1.1\r\n\r\n".as_slice(),
        ];
        for request in cases {
            assert_eq!(app().handle(request).status(), 400);
        }
    }

    #[test]
    fn oversized_or_ambiguous_bodies_are_rejected_before_actor_call() {
        let oversized =
            b"POST /api/start HTTP/1.1\r\nHost: localhost:8096\r\nContent-Length: 4097\r\n\r\n";
        let mut app = app();
        assert_eq!(app.handle(oversized).status(), 413);

        let csrf = token();
        let bad_json = String::from_utf8(post("/api/start", 0, &csrf, &csrf))
            .unwrap()
            .replace("Content-Length: 2", "Content-Length: 4")
            .replace("\r\n\r\n{}", "\r\n\r\nnull");
        assert_eq!(app.handle(bad_json.as_bytes()).status(), 400);
        assert_eq!(app.into_actor().mutations, 0);
    }

    #[test]
    fn content_type_length_and_strong_if_match_are_mandatory() {
        let csrf = token();
        let base = String::from_utf8(post("/api/start", 0, &csrf, &csrf)).unwrap();
        let no_length = base.replace("Content-Length: 2\r\n", "");
        assert_eq!(app().handle(no_length.as_bytes()).status(), 411);

        let wrong_type = base.replace("application/json", "text/plain");
        assert_eq!(app().handle(wrong_type.as_bytes()).status(), 415);

        let weak = base.replace("If-Match: \"0\"", "If-Match: W/\"0\"");
        assert_eq!(app().handle(weak.as_bytes()).status(), 428);
    }

    #[test]
    fn unsupported_method_path_and_version_are_closed() {
        assert_eq!(
            app()
                .handle(b"PUT /api/start HTTP/1.1\r\nHost: localhost:8096\r\n\r\n")
                .status(),
            405
        );
        assert_eq!(
            app()
                .handle(&get("/api/recover", "localhost:8096"))
                .status(),
            404
        );
        assert_eq!(
            app()
                .handle(b"GET / HTTP/1.0\r\nHost: localhost:8096\r\n\r\n")
                .status(),
            505
        );
    }
}
