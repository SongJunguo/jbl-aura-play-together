//! Bounded GENA observer for OneOS 7951 broadcaster business results.
//!
//! The observer is armed before the corresponding 7957 write. It binds one
//! fixed callback port on the IPv4 interface selected by the route to the exact
//! configured JBL, creates a fresh high-entropy callback path, and accepts only
//! a matching GENA subscription and ordered notifications from that JBL.
//! Diagnostics never contain addresses, paths, SIDs, or event bodies.
//!
//! Envelope boundary: no exact-firmware 7951 GENA body has yet been captured.
//! The bounded parser accepts only two independently expressed shapes derived
//! from the official application's event model: a direct UPnP PassThrough
//! property, or that property inside LastChange XML. The official snake_case
//! `pass_through.pass_string` plus JSON `payload` form is preferred; the
//! equivalent camelCase typed-object form is a strict compatibility shape.
//! Synthetic fixtures exercise both. Neither is claimed live-confirmed until
//! a real callback is captured.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use openssl::rand::rand_bytes;
use zeroize::Zeroizing;

const EVENT_PORT: u16 = 59_152;
#[cfg(test)]
const CALLBACK_PORT: u16 = 8_098;
const EVENT_PATH: &str = "/upnp/event/rendercontrol1";
const EVENT_NAMESPACE: &str = "urn:schemas-upnp-org:event-1-0";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const BUSINESS_TIMEOUT: Duration = Duration::from_secs(15);
const SUBSCRIPTION_SECONDS: u16 = 30;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADER_COUNT: usize = 64;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_CALLBACKS: usize = 64;
const MAX_XML_NODES: usize = 256;
const MAX_XML_DEPTH: usize = 16;
const MAX_XML_TEXT_BYTES: usize = 64 * 1024;
const MAX_JSON_NODES: usize = 256;
const MAX_JSON_DEPTH: usize = 16;
const MAX_JSON_KEY_BYTES: usize = 128;
const MAX_SID_BYTES: usize = 256;
const MIN_CALLBACK_PORT: u16 = 1_024;
const CALLBACK_PREFIX: &str = "/jbl-aura-event/";
const NOTIFICATION_COMMAND: &str = "notifyDeviceAuracastBroadcastInfo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenaAction {
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenaFailure {
    InvalidConfiguration,
    RouteUnavailable,
    ListenerUnavailable,
    SubscriptionUnavailable,
    InvalidSubscription,
    CallbackTimedOut,
    InvalidCallback,
    BusinessRejected,
    CleanupFailed,
}

impl GenaFailure {
    const fn label(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_configuration",
            Self::RouteUnavailable => "route_unavailable",
            Self::ListenerUnavailable => "listener_unavailable",
            Self::SubscriptionUnavailable => "subscription_unavailable",
            Self::InvalidSubscription => "invalid_subscription",
            Self::CallbackTimedOut => "callback_timed_out",
            Self::InvalidCallback => "invalid_callback",
            Self::BusinessRejected => "business_rejected",
            Self::CleanupFailed => "cleanup_failed",
        }
    }
}

impl fmt::Display for GenaFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

impl std::error::Error for GenaFailure {}

pub(crate) struct GenaBroadcastObserver {
    jbl_address: Ipv4Addr,
    event_port: u16,
    callback_port: u16,
    allow_loopback: bool,
    subscription: Option<Subscription>,
}

impl GenaBroadcastObserver {
    /// Creates an observer for one exact JBL IPv4 and the project's fixed
    /// callback port. The production path rejects loopback and non-unicast IPs.
    #[cfg(test)]
    pub(crate) fn new(address: &str) -> Result<Self, GenaFailure> {
        Self::with_callback_port(address, CALLBACK_PORT)
    }

    /// Creates the same exact-IP observer with an operator-selected port that
    /// remains fixed for the observer lifetime and its narrow firewall rule.
    pub(crate) fn with_callback_port(
        address: &str,
        callback_port: u16,
    ) -> Result<Self, GenaFailure> {
        let jbl_address = address
            .parse::<Ipv4Addr>()
            .map_err(|_| GenaFailure::InvalidConfiguration)?;
        Self::from_parts(jbl_address, EVENT_PORT, callback_port, false)
    }

    fn from_parts(
        jbl_address: Ipv4Addr,
        event_port: u16,
        callback_port: u16,
        allow_loopback: bool,
    ) -> Result<Self, GenaFailure> {
        if event_port == 0
            || callback_port < MIN_CALLBACK_PORT
            || jbl_address.is_unspecified()
            || jbl_address.is_multicast()
            || jbl_address == Ipv4Addr::BROADCAST
            || (jbl_address.is_loopback() && !allow_loopback)
        {
            return Err(GenaFailure::InvalidConfiguration);
        }
        Ok(Self {
            jbl_address,
            event_port,
            callback_port,
            allow_loopback,
            subscription: None,
        })
    }

    /// Arms the callback before a 7957 write. Re-arming first tears down any
    /// prior subscription and refuses to continue if cleanup is uncertain.
    pub(crate) fn arm(&mut self) -> Result<(), GenaFailure> {
        self.cancel()?;
        let local_address = select_local_ipv4(self.jbl_address, self.event_port)?;
        if local_address.is_unspecified()
            || local_address.is_multicast()
            || (local_address.is_loopback() && !self.allow_loopback)
        {
            return Err(GenaFailure::RouteUnavailable);
        }

        let listener = TcpListener::bind(SocketAddrV4::new(local_address, self.callback_port))
            .map_err(|_| GenaFailure::ListenerUnavailable)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| GenaFailure::ListenerUnavailable)?;
        let path = random_callback_path()?;
        let callback = format!(
            "<http://{local_address}:{}{}>",
            self.callback_port,
            path.as_str()
        );
        let target = SocketAddrV4::new(self.jbl_address, self.event_port);
        let response = event_request(
            target,
            "SUBSCRIBE",
            &[
                ("CALLBACK", callback),
                ("NT", "upnp:event".to_string()),
                ("TIMEOUT", format!("Second-{SUBSCRIPTION_SECONDS}")),
            ],
            GenaFailure::SubscriptionUnavailable,
        )?;
        if response.status() != Some(200) {
            return Err(GenaFailure::InvalidSubscription);
        }
        let sid = response
            .header("sid")
            .filter(|value| valid_sid(value))
            .ok_or(GenaFailure::InvalidSubscription)?;
        if response
            .header("timeout")
            .is_some_and(|timeout| !valid_subscription_timeout(timeout))
        {
            let _ = event_request(
                target,
                "UNSUBSCRIBE",
                &[("SID", sid.to_string())],
                GenaFailure::CleanupFailed,
            );
            return Err(GenaFailure::InvalidSubscription);
        }

        self.subscription = Some(Subscription {
            listener,
            sid: Zeroizing::new(sid.to_string()),
            path,
            last_seq: None,
        });
        Ok(())
    }

    /// Waits at most 15 seconds for the action-specific result, then always
    /// attempts UNSUBSCRIBE before returning.
    pub(crate) fn observe(&mut self, action: GenaAction) -> Result<(), GenaFailure> {
        self.observe_for(action, BUSINESS_TIMEOUT)
    }

    fn observe_for(&mut self, action: GenaAction, timeout: Duration) -> Result<(), GenaFailure> {
        let deadline = Instant::now() + timeout.min(BUSINESS_TIMEOUT);
        let result = self.observe_until(action, deadline);
        let cleanup = self.cancel();
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(_)) => Err(GenaFailure::CleanupFailed),
            (Err(error), _) => Err(error),
        }
    }

    fn observe_until(&mut self, action: GenaAction, deadline: Instant) -> Result<(), GenaFailure> {
        let subscription = self
            .subscription
            .as_mut()
            .ok_or(GenaFailure::ListenerUnavailable)?;
        let mut accepted = 0_usize;
        while accepted < MAX_CALLBACKS {
            if Instant::now() >= deadline {
                return Err(GenaFailure::CallbackTimedOut);
            }
            match subscription.listener.accept() {
                Ok((mut stream, peer)) => {
                    accepted += 1;
                    if peer.ip() != std::net::IpAddr::V4(self.jbl_address) {
                        continue;
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let io_timeout = remaining.min(CONNECT_TIMEOUT);
                    stream
                        .set_read_timeout(Some(io_timeout))
                        .map_err(|_| GenaFailure::InvalidCallback)?;
                    stream
                        .set_write_timeout(Some(io_timeout))
                        .map_err(|_| GenaFailure::InvalidCallback)?;
                    let request = match read_http_message(&mut stream, MessageKind::Request, true) {
                        Ok(request) => request,
                        Err(error) => {
                            write_callback_response(&mut stream, false);
                            return Err(error);
                        }
                    };
                    let outcome = validate_callback(
                        &request,
                        subscription.path.as_str(),
                        subscription.sid.as_str(),
                        &mut subscription.last_seq,
                        action,
                    );
                    write_callback_response(&mut stream, outcome.is_ok());
                    match outcome? {
                        CallbackOutcome::Confirmed => return Ok(()),
                        CallbackOutcome::Rejected => return Err(GenaFailure::BusinessRejected),
                        CallbackOutcome::Pending => continue,
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return Err(GenaFailure::ListenerUnavailable),
            }
        }
        Err(GenaFailure::InvalidCallback)
    }

    /// Closes the listener before the bounded network cleanup request, so no
    /// callback stays exposed if UNSUBSCRIBE itself fails.
    pub(crate) fn cancel(&mut self) -> Result<(), GenaFailure> {
        let Some(subscription) = self.subscription.take() else {
            return Ok(());
        };
        drop(subscription.listener);
        let response = event_request(
            SocketAddrV4::new(self.jbl_address, self.event_port),
            "UNSUBSCRIBE",
            &[("SID", subscription.sid.to_string())],
            GenaFailure::CleanupFailed,
        )?;
        if response.status() == Some(200) {
            Ok(())
        } else {
            Err(GenaFailure::CleanupFailed)
        }
    }
}

impl Drop for GenaBroadcastObserver {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

impl fmt::Debug for GenaBroadcastObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenaBroadcastObserver")
            .field("subscription", &self.subscription.is_some())
            .field("endpoint", &"redacted")
            .finish()
    }
}

struct Subscription {
    listener: TcpListener,
    sid: Zeroizing<String>,
    path: Zeroizing<String>,
    last_seq: Option<u32>,
}

#[derive(Clone, Copy)]
enum MessageKind {
    Request,
    Response,
}

enum StartLine {
    Request { method: String, target: String },
    Response { status: u16 },
}

struct HttpMessage {
    start_line: StartLine,
    headers: BTreeMap<String, String>,
    body: Zeroizing<Vec<u8>>,
}

impl HttpMessage {
    fn status(&self) -> Option<u16> {
        match self.start_line {
            StartLine::Response { status } => Some(status),
            StartLine::Request { .. } => None,
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

fn select_local_ipv4(jbl: Ipv4Addr, event_port: u16) -> Result<Ipv4Addr, GenaFailure> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|_| GenaFailure::RouteUnavailable)?;
    socket
        .connect(SocketAddrV4::new(jbl, event_port))
        .map_err(|_| GenaFailure::RouteUnavailable)?;
    match socket
        .local_addr()
        .map_err(|_| GenaFailure::RouteUnavailable)?
    {
        SocketAddr::V4(address) => Ok(*address.ip()),
        SocketAddr::V6(_) => Err(GenaFailure::RouteUnavailable),
    }
}

fn event_request(
    target: SocketAddrV4,
    method: &str,
    extra_headers: &[(&str, String)],
    failure: GenaFailure,
) -> Result<HttpMessage, GenaFailure> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::V4(target), CONNECT_TIMEOUT)
        .map_err(|_| failure)?;
    stream
        .set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|_| failure)?;
    stream
        .set_write_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|_| failure)?;
    write!(
        stream,
        "{method} {EVENT_PATH} HTTP/1.1\r\nHOST: {target}\r\nContent-Length: 0\r\nConnection: close\r\n"
    )
    .map_err(|_| failure)?;
    for (name, value) in extra_headers {
        write!(stream, "{name}: {value}\r\n").map_err(|_| failure)?;
    }
    stream.write_all(b"\r\n").map_err(|_| failure)?;
    stream.flush().map_err(|_| failure)?;
    read_http_message(&mut stream, MessageKind::Response, false).map_err(|_| failure)
}

fn read_http_message(
    stream: &mut impl Read,
    kind: MessageKind,
    content_length_required: bool,
) -> Result<HttpMessage, GenaFailure> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let (header_end, start_line, headers, content_length) = loop {
        let count = stream
            .read(&mut buffer)
            .map_err(|_| GenaFailure::InvalidCallback)?;
        if count == 0 {
            return Err(GenaFailure::InvalidCallback);
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > MAX_HEADER_BYTES + 4 + MAX_BODY_BYTES {
            return Err(GenaFailure::InvalidCallback);
        }
        if let Some(index) = find_subsequence(&bytes, b"\r\n\r\n") {
            if index > MAX_HEADER_BYTES {
                return Err(GenaFailure::InvalidCallback);
            }
            let (start_line, headers, content_length) =
                parse_http_head(&bytes[..index], kind, content_length_required)?;
            break (index + 4, start_line, headers, content_length);
        }
        if bytes.len() > MAX_HEADER_BYTES + 4 {
            return Err(GenaFailure::InvalidCallback);
        }
    };
    let total = header_end
        .checked_add(content_length)
        .ok_or(GenaFailure::InvalidCallback)?;
    if bytes.len() > total {
        return Err(GenaFailure::InvalidCallback);
    }
    while bytes.len() < total {
        let count = stream
            .read(&mut buffer)
            .map_err(|_| GenaFailure::InvalidCallback)?;
        if count == 0 {
            return Err(GenaFailure::InvalidCallback);
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > total {
            return Err(GenaFailure::InvalidCallback);
        }
    }
    Ok(HttpMessage {
        start_line,
        headers,
        body: Zeroizing::new(bytes[header_end..].to_vec()),
    })
}

fn parse_http_head(
    bytes: &[u8],
    kind: MessageKind,
    content_length_required: bool,
) -> Result<(StartLine, BTreeMap<String, String>, usize), GenaFailure> {
    let head = std::str::from_utf8(bytes).map_err(|_| GenaFailure::InvalidCallback)?;
    let mut lines = head.split("\r\n");
    let first = lines.next().ok_or(GenaFailure::InvalidCallback)?;
    let start_line = parse_start_line(first, kind)?;
    let mut headers = BTreeMap::new();
    for (index, line) in lines.enumerate() {
        if index >= MAX_HEADER_COUNT || line.is_empty() || line.starts_with([' ', '\t']) {
            return Err(GenaFailure::InvalidCallback);
        }
        let (name, value) = line.split_once(':').ok_or(GenaFailure::InvalidCallback)?;
        if name.is_empty() || !name.bytes().all(is_http_token_byte) {
            return Err(GenaFailure::InvalidCallback);
        }
        let value = value.trim_matches([' ', '\t']);
        if value
            .bytes()
            .any(|byte| byte == 0x7f || (byte < 0x20 && byte != b'\t'))
        {
            return Err(GenaFailure::InvalidCallback);
        }
        // Reject every duplicate. This is stricter than the sensitive-header
        // minimum and closes conflicting Content-Length/SID/SEQ semantics.
        if headers
            .insert(name.to_ascii_lowercase(), value.to_string())
            .is_some()
        {
            return Err(GenaFailure::InvalidCallback);
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err(GenaFailure::InvalidCallback);
    }
    let content_length = match headers.get("content-length") {
        Some(value) => parse_decimal_usize(value)?,
        None if content_length_required => return Err(GenaFailure::InvalidCallback),
        None => 0,
    };
    if content_length > MAX_BODY_BYTES {
        return Err(GenaFailure::InvalidCallback);
    }
    Ok((start_line, headers, content_length))
}

fn parse_start_line(line: &str, kind: MessageKind) -> Result<StartLine, GenaFailure> {
    match kind {
        MessageKind::Request => {
            let mut parts = line.split(' ');
            let method = parts.next().ok_or(GenaFailure::InvalidCallback)?;
            let target = parts.next().ok_or(GenaFailure::InvalidCallback)?;
            let version = parts.next().ok_or(GenaFailure::InvalidCallback)?;
            if parts.next().is_some()
                || method.is_empty()
                || target.is_empty()
                || version != "HTTP/1.1"
                || !method.bytes().all(is_http_token_byte)
            {
                return Err(GenaFailure::InvalidCallback);
            }
            Ok(StartLine::Request {
                method: method.to_string(),
                target: target.to_string(),
            })
        }
        MessageKind::Response => {
            let mut parts = line.splitn(3, ' ');
            if parts.next() != Some("HTTP/1.1") {
                return Err(GenaFailure::InvalidCallback);
            }
            let status = parts
                .next()
                .filter(|value| value.len() == 3 && value.bytes().all(|b| b.is_ascii_digit()))
                .ok_or(GenaFailure::InvalidCallback)?
                .parse::<u16>()
                .map_err(|_| GenaFailure::InvalidCallback)?;
            if !(100..=599).contains(&status) {
                return Err(GenaFailure::InvalidCallback);
            }
            if parts.next().is_some_and(|reason| {
                reason
                    .bytes()
                    .any(|byte| byte == 0x7f || (byte < 0x20 && byte != b'\t'))
            }) {
                return Err(GenaFailure::InvalidCallback);
            }
            Ok(StartLine::Response { status })
        }
    }
}

fn parse_decimal_usize(value: &str) -> Result<usize, GenaFailure> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(GenaFailure::InvalidCallback);
    }
    value
        .parse::<usize>()
        .map_err(|_| GenaFailure::InvalidCallback)
}

fn is_http_token_byte(byte: u8) -> bool {
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

fn write_callback_response(stream: &mut TcpStream, accepted: bool) {
    let status = if accepted {
        "200 OK"
    } else {
        "400 Bad Request"
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.flush();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallbackOutcome {
    Confirmed,
    Rejected,
    Pending,
}

fn validate_callback(
    request: &HttpMessage,
    path: &str,
    sid: &str,
    last_seq: &mut Option<u32>,
    action: GenaAction,
) -> Result<CallbackOutcome, GenaFailure> {
    let StartLine::Request { method, target } = &request.start_line else {
        return Err(GenaFailure::InvalidCallback);
    };
    if method != "NOTIFY" || target != path {
        return Err(GenaFailure::InvalidCallback);
    }
    if request.header("sid") != Some(sid)
        || !header_eq_ignore_ascii_case(request.header("nt"), "upnp:event")
        || !header_eq_ignore_ascii_case(request.header("nts"), "upnp:propchange")
    {
        return Err(GenaFailure::InvalidCallback);
    }
    if let Some(content_type) = request.header("content-type") {
        let media_type = content_type
            .split(';')
            .next()
            .map(str::trim)
            .unwrap_or_default();
        if !media_type.eq_ignore_ascii_case("text/xml")
            && !media_type.eq_ignore_ascii_case("application/xml")
        {
            return Err(GenaFailure::InvalidCallback);
        }
    }
    let seq_text = request.header("seq").ok_or(GenaFailure::InvalidCallback)?;
    if seq_text.is_empty() || !seq_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(GenaFailure::InvalidCallback);
    }
    let seq = seq_text
        .parse::<u32>()
        .map_err(|_| GenaFailure::InvalidCallback)?;
    if !sequence_is_next(*last_seq, seq) {
        return Err(GenaFailure::InvalidCallback);
    }
    let action_code = parse_event_body(request.body.as_slice())?;
    *last_seq = Some(seq);
    Ok(reduce_action(action, action_code))
}

fn sequence_is_next(previous: Option<u32>, next: u32) -> bool {
    match previous {
        // A fresh SID plus path binds the subscription; the first server event
        // key need not be guessed as zero.
        None => true,
        // UPnP event keys roll from 2^32-1 to 1; zero is reserved after wrap.
        Some(u32::MAX) => next == 1,
        Some(previous) => next == previous + 1,
    }
}

fn header_eq_ignore_ascii_case(value: Option<&str>, expected: &str) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn reduce_action(action: GenaAction, action_code: Option<i64>) -> CallbackOutcome {
    match (action, action_code) {
        (GenaAction::Start, Some(33)) | (GenaAction::Stop, Some(34)) => CallbackOutcome::Confirmed,
        (GenaAction::Start, Some(31)) | (GenaAction::Stop, Some(32)) => CallbackOutcome::Rejected,
        _ => CallbackOutcome::Pending,
    }
}

fn parse_event_body(body: &[u8]) -> Result<Option<i64>, GenaFailure> {
    let xml = std::str::from_utf8(body).map_err(|_| GenaFailure::InvalidCallback)?;
    let document = parse_bounded_xml(xml)?;
    let root = document.root_element();
    if root.tag_name().name() != "propertyset"
        || root.tag_name().namespace() != Some(EVENT_NAMESPACE)
    {
        return Err(GenaFailure::InvalidCallback);
    }
    let mut result = None;
    for property in root.children().filter(roxmltree::Node::is_element) {
        if property.tag_name().name() != "property" {
            return Err(GenaFailure::InvalidCallback);
        }
        let mut values = property.children().filter(roxmltree::Node::is_element);
        let value = values.next().ok_or(GenaFailure::InvalidCallback)?;
        if values.next().is_some() {
            return Err(GenaFailure::InvalidCallback);
        }
        let candidate = match value.tag_name().name() {
            "UpnpNotifyPassThrough" | "PassThrough" => parse_pass_through_node(value)?,
            "LastChange" => parse_last_change_node(value)?,
            _ => None,
        };
        merge_action_code(&mut result, candidate)?;
    }
    Ok(result)
}

fn parse_bounded_xml(xml: &str) -> Result<roxmltree::Document<'_>, GenaFailure> {
    if xml.len() > MAX_BODY_BYTES
        || xml.contains("<!DOCTYPE")
        || xml.contains("<!doctype")
        || xml.contains("<!ENTITY")
        || xml.contains("<!entity")
    {
        return Err(GenaFailure::InvalidCallback);
    }
    let document = roxmltree::Document::parse(xml).map_err(|_| GenaFailure::InvalidCallback)?;
    validate_xml_limits(&document)?;
    Ok(document)
}

fn validate_xml_limits(document: &roxmltree::Document<'_>) -> Result<(), GenaFailure> {
    let mut nodes = 0_usize;
    let mut text_bytes = 0_usize;
    for node in document.descendants() {
        nodes = nodes.checked_add(1).ok_or(GenaFailure::InvalidCallback)?;
        if nodes > MAX_XML_NODES
            || node.ancestors().filter(roxmltree::Node::is_element).count() > MAX_XML_DEPTH
        {
            return Err(GenaFailure::InvalidCallback);
        }
        if node.is_text() {
            let text = node.text().ok_or(GenaFailure::InvalidCallback)?;
            text_bytes = text_bytes
                .checked_add(text.len())
                .ok_or(GenaFailure::InvalidCallback)?;
            if text_bytes > MAX_XML_TEXT_BYTES {
                return Err(GenaFailure::InvalidCallback);
            }
        }
    }
    Ok(())
}

fn parse_pass_through_node(node: roxmltree::Node<'_, '_>) -> Result<Option<i64>, GenaFailure> {
    parse_notification_json(node_payload(node)?)
}

fn parse_last_change_node(node: roxmltree::Node<'_, '_>) -> Result<Option<i64>, GenaFailure> {
    if node.children().any(|child| child.is_element()) {
        return inspect_nested_pass_through(node);
    }
    let text = collect_direct_text(node)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.starts_with('{') {
        return parse_notification_json(trimmed);
    }
    if !trimmed.starts_with('<') {
        return Ok(None);
    }
    let nested = parse_bounded_xml(trimmed)?;
    inspect_nested_pass_through(nested.root_element())
}

fn inspect_nested_pass_through(root: roxmltree::Node<'_, '_>) -> Result<Option<i64>, GenaFailure> {
    let mut result = None;
    for node in root.descendants().filter(roxmltree::Node::is_element) {
        if matches!(
            node.tag_name().name(),
            "UpnpNotifyPassThrough" | "PassThrough"
        ) {
            merge_action_code(&mut result, parse_pass_through_node(node)?)?;
        }
    }
    Ok(result)
}

fn node_payload<'a>(node: roxmltree::Node<'a, '_>) -> Result<&'a str, GenaFailure> {
    let attribute = match (node.attribute("val"), node.attribute("value")) {
        (Some(_), Some(_)) => return Err(GenaFailure::InvalidCallback),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    };
    if let Some(value) = attribute {
        if node.children().any(|child| child.is_element()) || value.len() > MAX_XML_TEXT_BYTES {
            return Err(GenaFailure::InvalidCallback);
        }
        return Ok(value.trim());
    }
    if node.children().any(|child| child.is_element()) {
        return Err(GenaFailure::InvalidCallback);
    }
    let text = node.text().ok_or(GenaFailure::InvalidCallback)?.trim();
    if text.is_empty() || text.len() > MAX_XML_TEXT_BYTES {
        return Err(GenaFailure::InvalidCallback);
    }
    Ok(text)
}

fn collect_direct_text(node: roxmltree::Node<'_, '_>) -> Result<String, GenaFailure> {
    let mut text = String::new();
    for child in node.children() {
        if child.is_element() {
            return Err(GenaFailure::InvalidCallback);
        }
        if child.is_text() {
            let value = child.text().ok_or(GenaFailure::InvalidCallback)?;
            if text.len().saturating_add(value.len()) > MAX_XML_TEXT_BYTES {
                return Err(GenaFailure::InvalidCallback);
            }
            text.push_str(value);
        }
    }
    Ok(text)
}

fn parse_notification_json(text: &str) -> Result<Option<i64>, GenaFailure> {
    if text.is_empty() || text.len() > MAX_BODY_BYTES {
        return Err(GenaFailure::InvalidCallback);
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| GenaFailure::InvalidCallback)?;
    validate_json_limits(&value)?;
    let root = value.get("UpnpNotifyPassThrough").unwrap_or(&value);
    let pass_through = root
        .get("pass_through")
        .or_else(|| root.get("passThrough"))
        .ok_or(GenaFailure::InvalidCallback)?;
    let pass_string = pass_through
        .get("pass_string")
        .or_else(|| pass_through.get("passString"))
        .ok_or(GenaFailure::InvalidCallback)?;
    let command = pass_string
        .get("command")
        .and_then(serde_json::Value::as_str)
        .ok_or(GenaFailure::InvalidCallback)?;
    if command != NOTIFICATION_COMMAND {
        return Ok(None);
    }
    if let Some(payload) = pass_string.get("payload") {
        return extract_payload_action_code(payload).map(Some);
    }
    extract_info_action_code(
        pass_string
            .get(NOTIFICATION_COMMAND)
            .ok_or(GenaFailure::InvalidCallback)?,
    )
    .map(Some)
}

fn extract_payload_action_code(payload: &serde_json::Value) -> Result<i64, GenaFailure> {
    match payload {
        serde_json::Value::String(text) => {
            if text.is_empty() || text.len() > MAX_BODY_BYTES {
                return Err(GenaFailure::InvalidCallback);
            }
            let parsed: serde_json::Value =
                serde_json::from_str(text).map_err(|_| GenaFailure::InvalidCallback)?;
            validate_json_limits(&parsed)?;
            extract_info_action_code(parsed.get(NOTIFICATION_COMMAND).unwrap_or(&parsed))
        }
        serde_json::Value::Object(_) => {
            extract_info_action_code(payload.get(NOTIFICATION_COMMAND).unwrap_or(payload))
        }
        _ => Err(GenaFailure::InvalidCallback),
    }
}

fn extract_info_action_code(value: &serde_json::Value) -> Result<i64, GenaFailure> {
    let action_code = value
        .get("info")
        .and_then(|value| value.get("action_code"))
        .and_then(serde_json::Value::as_i64)
        .ok_or(GenaFailure::InvalidCallback)?;
    if !(0..=i64::from(u8::MAX)).contains(&action_code) {
        return Err(GenaFailure::InvalidCallback);
    }
    Ok(action_code)
}

fn validate_json_limits(value: &serde_json::Value) -> Result<(), GenaFailure> {
    fn visit(
        value: &serde_json::Value,
        depth: usize,
        nodes: &mut usize,
        text_bytes: &mut usize,
    ) -> Result<(), GenaFailure> {
        *nodes = nodes.checked_add(1).ok_or(GenaFailure::InvalidCallback)?;
        if depth > MAX_JSON_DEPTH || *nodes > MAX_JSON_NODES {
            return Err(GenaFailure::InvalidCallback);
        }
        match value {
            serde_json::Value::String(text) => {
                *text_bytes = text_bytes
                    .checked_add(text.len())
                    .ok_or(GenaFailure::InvalidCallback)?;
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    visit(value, depth + 1, nodes, text_bytes)?;
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    if key.len() > MAX_JSON_KEY_BYTES {
                        return Err(GenaFailure::InvalidCallback);
                    }
                    *text_bytes = text_bytes
                        .checked_add(key.len())
                        .ok_or(GenaFailure::InvalidCallback)?;
                    visit(value, depth + 1, nodes, text_bytes)?;
                }
            }
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            }
        }
        if *text_bytes > MAX_BODY_BYTES {
            return Err(GenaFailure::InvalidCallback);
        }
        Ok(())
    }
    let mut nodes = 0_usize;
    let mut text_bytes = 0_usize;
    visit(value, 1, &mut nodes, &mut text_bytes)
}

fn merge_action_code(target: &mut Option<i64>, candidate: Option<i64>) -> Result<(), GenaFailure> {
    if let Some(candidate) = candidate {
        if target.replace(candidate).is_some() {
            return Err(GenaFailure::InvalidCallback);
        }
    }
    Ok(())
}

fn random_callback_path() -> Result<Zeroizing<String>, GenaFailure> {
    let mut random = [0_u8; 32];
    rand_bytes(&mut random).map_err(|_| GenaFailure::ListenerUnavailable)?;
    let mut path = Zeroizing::new(String::with_capacity(CALLBACK_PREFIX.len() + 64));
    path.push_str(CALLBACK_PREFIX);
    for byte in random {
        write!(&mut *path, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(path)
}

fn valid_sid(sid: &str) -> bool {
    sid.len() <= MAX_SID_BYTES
        && sid.len() > "uuid:".len()
        && sid.starts_with("uuid:")
        && sid.bytes().all(|byte| byte.is_ascii_graphic())
        && !sid
            .bytes()
            .any(|byte| matches!(byte, b'<' | b'>' | b',' | b';'))
}

fn valid_subscription_timeout(timeout: &str) -> bool {
    timeout
        .strip_prefix("Second-")
        .and_then(|seconds| seconds.parse::<u16>().ok())
        .is_some_and(|seconds| (1..=SUBSCRIPTION_SECONDS).contains(&seconds))
}

fn find_subsequence(payload: &[u8], needle: &[u8]) -> Option<usize> {
    payload
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    const PATH: &str =
        "/jbl-aura-event/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const SID: &str = "uuid:synthetic-subscription";

    fn observer_for_test(event_port: u16, callback_port: u16) -> GenaBroadcastObserver {
        GenaBroadcastObserver::from_parts(Ipv4Addr::LOCALHOST, event_port, callback_port, true)
            .expect("fixture observer")
    }

    fn snake_notification(action_code: i64) -> String {
        let payload = serde_json::json!({"info": {"action_code": action_code}}).to_string();
        serde_json::json!({
            "pass_through": {
                "pass_string": {
                    "command": NOTIFICATION_COMMAND,
                    "payload": payload
                }
            }
        })
        .to_string()
    }

    fn camel_notification(action_code: i64) -> String {
        serde_json::json!({
            "passThrough": {
                "passString": {
                    "command": NOTIFICATION_COMMAND,
                    NOTIFICATION_COMMAND: {"info": {"action_code": action_code}}
                }
            }
        })
        .to_string()
    }

    fn xml_escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    fn event_xml(action_code: i64) -> Vec<u8> {
        format!(
            "<e:propertyset xmlns:e=\"{EVENT_NAMESPACE}\"><e:property><UpnpNotifyPassThrough>{}</UpnpNotifyPassThrough></e:property></e:propertyset>",
            xml_escape(&snake_notification(action_code))
        )
        .into_bytes()
    }

    fn callback_message(path: &str, sid: &str, seq: u32, action_code: i64) -> HttpMessage {
        let body = event_xml(action_code);
        HttpMessage {
            start_line: StartLine::Request {
                method: "NOTIFY".to_string(),
                target: path.to_string(),
            },
            headers: BTreeMap::from([
                ("content-length".to_string(), body.len().to_string()),
                ("sid".to_string(), sid.to_string()),
                ("seq".to_string(), seq.to_string()),
                ("nt".to_string(), "upnp:event".to_string()),
                ("nts".to_string(), "upnp:propchange".to_string()),
            ]),
            body: Zeroizing::new(body),
        }
    }

    #[test]
    fn production_constructor_requires_one_exact_non_loopback_ipv4() {
        for invalid in [
            "::1",
            "0.0.0.0",
            "127.0.0.1",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert_eq!(
                GenaBroadcastObserver::new(invalid).unwrap_err(),
                GenaFailure::InvalidConfiguration
            );
        }
        assert!(GenaBroadcastObserver::new("192.0.2.10").is_ok());
        assert_eq!(
            GenaBroadcastObserver::with_callback_port("192.0.2.10", 80).unwrap_err(),
            GenaFailure::InvalidConfiguration
        );
    }

    #[test]
    fn reducer_is_action_specific_and_all_other_codes_stay_pending() {
        assert_eq!(
            reduce_action(GenaAction::Start, Some(33)),
            CallbackOutcome::Confirmed
        );
        assert_eq!(
            reduce_action(GenaAction::Start, Some(31)),
            CallbackOutcome::Rejected
        );
        assert_eq!(
            reduce_action(GenaAction::Stop, Some(34)),
            CallbackOutcome::Confirmed
        );
        assert_eq!(
            reduce_action(GenaAction::Stop, Some(32)),
            CallbackOutcome::Rejected
        );
        for code in [None, Some(30), Some(32), Some(34), Some(99)] {
            assert_eq!(
                reduce_action(GenaAction::Start, code),
                CallbackOutcome::Pending
            );
        }
    }

    #[test]
    fn direct_property_decodes_the_official_snake_case_payload_shape() {
        assert_eq!(parse_event_body(&event_xml(33)), Ok(Some(33)));
    }

    #[test]
    fn last_change_decodes_one_bounded_nested_pass_through_shape() {
        let nested = format!(
            "<Event><InstanceID><PassThrough>{}</PassThrough></InstanceID></Event>",
            xml_escape(&camel_notification(34))
        );
        let outer = format!(
            "<e:propertyset xmlns:e=\"{EVENT_NAMESPACE}\"><e:property><LastChange>{}</LastChange></e:property></e:propertyset>",
            xml_escape(&nested)
        );
        assert_eq!(parse_event_body(outer.as_bytes()), Ok(Some(34)));
    }

    #[test]
    fn malformed_ambiguous_or_deep_xml_is_closed() {
        let escaped = xml_escape(&snake_notification(33));
        let duplicate = format!(
            "<e:propertyset xmlns:e=\"{EVENT_NAMESPACE}\"><e:property><PassThrough>{escaped}</PassThrough></e:property><e:property><PassThrough>{escaped}</PassThrough></e:property></e:propertyset>"
        );
        assert_eq!(
            parse_event_body(duplicate.as_bytes()),
            Err(GenaFailure::InvalidCallback)
        );

        let mut nested = String::new();
        for _ in 0..=MAX_XML_DEPTH {
            nested.push_str("<x>");
        }
        for _ in 0..=MAX_XML_DEPTH {
            nested.push_str("</x>");
        }
        let deep = format!(
            "<e:propertyset xmlns:e=\"{EVENT_NAMESPACE}\"><e:property><LastChange>{}</LastChange></e:property></e:propertyset>",
            xml_escape(&nested)
        );
        assert_eq!(
            parse_event_body(deep.as_bytes()),
            Err(GenaFailure::InvalidCallback)
        );
    }

    #[test]
    fn http_parser_rejects_duplicates_chunking_missing_length_and_oversize() {
        for head in [
            "NOTIFY / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1",
            "NOTIFY / HTTP/1.1\r\nTransfer-Encoding: chunked",
            "NOTIFY / HTTP/1.1\r\nSID: uuid:a\r\nSID: uuid:b\r\nContent-Length: 0",
        ] {
            assert!(parse_http_head(head.as_bytes(), MessageKind::Request, false).is_err());
        }
        assert!(parse_http_head(b"NOTIFY / HTTP/1.1", MessageKind::Request, true).is_err());
        let oversized = format!(
            "NOTIFY / HTTP/1.1\r\nContent-Length: {}",
            MAX_BODY_BYTES + 1
        );
        assert!(parse_http_head(oversized.as_bytes(), MessageKind::Request, true).is_err());
        assert!(parse_http_head(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0",
            MessageKind::Response,
            false
        )
        .is_ok());
    }

    #[test]
    fn callback_requires_exact_path_sid_and_strictly_ordered_sequence() {
        let mut seq = None;
        assert_eq!(
            validate_callback(
                &callback_message(PATH, SID, 41, 99),
                PATH,
                SID,
                &mut seq,
                GenaAction::Start
            ),
            Ok(CallbackOutcome::Pending)
        );
        assert_eq!(seq, Some(41));
        assert_eq!(
            validate_callback(
                &callback_message(PATH, SID, 42, 33),
                PATH,
                SID,
                &mut seq,
                GenaAction::Start
            ),
            Ok(CallbackOutcome::Confirmed)
        );
        assert_eq!(seq, Some(42));

        for (path, sid, event_seq) in [
            ("/wrong", SID, 43),
            (PATH, "uuid:wrong", 43),
            (PATH, SID, 42),
            (PATH, SID, 44),
        ] {
            assert_eq!(
                validate_callback(
                    &callback_message(PATH, SID, event_seq, 33),
                    path,
                    sid,
                    &mut seq,
                    GenaAction::Start
                ),
                Err(GenaFailure::InvalidCallback)
            );
        }
        assert!(sequence_is_next(Some(u32::MAX), 1));
        assert!(!sequence_is_next(Some(u32::MAX), 0));
    }

    #[test]
    fn callback_paths_are_high_entropy_and_diagnostics_are_redacted() {
        let first = random_callback_path().expect("random path");
        let second = random_callback_path().expect("random path");
        assert!(first.starts_with(CALLBACK_PREFIX));
        assert_eq!(first.len(), CALLBACK_PREFIX.len() + 64);
        assert_ne!(first.as_str(), second.as_str());
        assert!(valid_sid(SID));
        assert!(!valid_sid("not-a-sid"));
        assert!(valid_subscription_timeout("Second-30"));
        assert!(!valid_subscription_timeout("Second-infinite"));
        let observer = observer_for_test(1, 18_098);
        assert_eq!(
            format!("{observer:?}"),
            "GenaBroadcastObserver { subscription: false, endpoint: \"redacted\" }"
        );
        assert_eq!(
            GenaFailure::CallbackTimedOut.to_string(),
            "callback_timed_out"
        );
    }

    #[derive(Clone, Copy)]
    enum FakeResult {
        Confirm,
        Reject,
        Silence,
    }

    fn unused_loopback_port() -> u16 {
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .expect("ephemeral listener")
            .local_addr()
            .expect("ephemeral address")
            .port()
    }

    fn accept_until(listener: &TcpListener, deadline: Instant) -> TcpStream {
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "fake GENA accept timed out");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("fake GENA accept failed: {error}"),
            }
        }
    }

    fn spawn_fake_gena_server(
        result: FakeResult,
        callback_port: u16,
    ) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .expect("fake GENA listener");
        let event_port = listener.local_addr().expect("fake GENA address").port();
        listener.set_nonblocking(true).expect("fake nonblocking");
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut subscribe = accept_until(&listener, deadline);
            subscribe
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("subscribe timeout");
            let request = read_http_message(&mut subscribe, MessageKind::Request, false)
                .expect("SUBSCRIBE request");
            let StartLine::Request { method, target } = &request.start_line else {
                panic!("expected request");
            };
            assert_eq!(method, "SUBSCRIBE");
            assert_eq!(target, EVENT_PATH);
            assert_eq!(request.header("nt"), Some("upnp:event"));
            assert_eq!(request.header("timeout"), Some("Second-30"));
            let callback = request.header("callback").expect("CALLBACK header");
            let prefix = format!("<http://127.0.0.1:{callback_port}");
            let path = callback
                .strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix('>'))
                .expect("bounded callback URL")
                .to_string();
            assert!(path.starts_with(CALLBACK_PREFIX));
            assert_eq!(path.len(), CALLBACK_PREFIX.len() + 64);
            write!(
                subscribe,
                "HTTP/1.1 200 OK\r\nSID: {SID}\r\nTIMEOUT: Second-30\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("SUBSCRIBE response");
            subscribe.flush().expect("SUBSCRIBE flush");
            drop(subscribe);

            let codes: &[i64] = match result {
                FakeResult::Confirm => &[99, 33],
                FakeResult::Reject => &[31],
                FakeResult::Silence => &[],
            };
            for (offset, action_code) in codes.iter().enumerate() {
                let mut callback_stream = loop {
                    match TcpStream::connect_timeout(
                        &SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, callback_port)),
                        Duration::from_millis(200),
                    ) {
                        Ok(stream) => break stream,
                        Err(_) if Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("fake callback failed: {error}"),
                    }
                };
                callback_stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .expect("callback timeout");
                let body = event_xml(*action_code);
                write!(
                    callback_stream,
                    "NOTIFY {path} HTTP/1.1\r\nHOST: 127.0.0.1:{callback_port}\r\nCONTENT-TYPE: text/xml; charset=utf-8\r\nNT: upnp:event\r\nNTS: upnp:propchange\r\nSID: {SID}\r\nSEQ: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    41 + offset,
                    body.len()
                )
                .expect("callback head");
                callback_stream.write_all(&body).expect("callback body");
                callback_stream.flush().expect("callback flush");
                let response =
                    read_http_message(&mut callback_stream, MessageKind::Response, false)
                        .expect("callback response");
                assert_eq!(response.status(), Some(200));
            }

            let mut unsubscribe = accept_until(&listener, deadline);
            unsubscribe
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("unsubscribe timeout");
            let request = read_http_message(&mut unsubscribe, MessageKind::Request, false)
                .expect("UNSUBSCRIBE request");
            let StartLine::Request { method, target } = &request.start_line else {
                panic!("expected request");
            };
            assert_eq!(method, "UNSUBSCRIBE");
            assert_eq!(target, EVENT_PATH);
            assert_eq!(request.header("sid"), Some(SID));
            write!(
                unsubscribe,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("UNSUBSCRIBE response");
            unsubscribe.flush().expect("UNSUBSCRIBE flush");
        });
        (event_port, handle)
    }

    #[test]
    fn fake_gena_server_confirms_after_pending_and_observes_final_unsubscribe() {
        let callback_port = unused_loopback_port();
        let (event_port, server) = spawn_fake_gena_server(FakeResult::Confirm, callback_port);
        let mut observer = observer_for_test(event_port, callback_port);
        observer.arm().expect("armed before synthetic write");
        assert_eq!(observer.observe(GenaAction::Start), Ok(()));
        server.join().expect("fake GENA server");
    }

    #[test]
    fn fake_gena_server_rejection_is_action_specific_and_still_unsubscribes() {
        let callback_port = unused_loopback_port();
        let (event_port, server) = spawn_fake_gena_server(FakeResult::Reject, callback_port);
        let mut observer = observer_for_test(event_port, callback_port);
        observer.arm().expect("armed before synthetic write");
        assert_eq!(
            observer.observe(GenaAction::Start),
            Err(GenaFailure::BusinessRejected)
        );
        server.join().expect("fake GENA server");
    }

    #[test]
    fn fake_gena_server_timeout_is_bounded_and_still_unsubscribes() {
        let callback_port = unused_loopback_port();
        let (event_port, server) = spawn_fake_gena_server(FakeResult::Silence, callback_port);
        let mut observer = observer_for_test(event_port, callback_port);
        observer.arm().expect("armed before synthetic write");
        assert_eq!(
            observer.observe_for(GenaAction::Stop, Duration::from_millis(40)),
            Err(GenaFailure::CallbackTimedOut)
        );
        server.join().expect("fake GENA server");
    }
}
