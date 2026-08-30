//! Linux Avahi D-Bus adapter for the privacy-safe discovery core.

use std::collections::VecDeque;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dbus::blocking::Connection;
use dbus::channel::{MatchingReceiver, Sender, Token};
use dbus::message::MatchRule;
use dbus::{Message, MessageType, Path};

use crate::discovery::{
    CandidateCollector, DiscoveryError, DiscoveryOptions, DiscoverySummary, ResolvedMdnsRecord,
    JBL_MDNS_SERVICE_TYPE,
};

const AVAHI_BUS: &str = "org.freedesktop.Avahi";
const AVAHI_SERVER_PATH: &str = "/";
const AVAHI_SERVER_INTERFACE: &str = "org.freedesktop.Avahi.Server";
const AVAHI_BROWSER_INTERFACE: &str = "org.freedesktop.Avahi.ServiceBrowser";
const DBUS_BUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const AVAHI_SERVICE_TYPE: &str = "_jbl-product._tcp";
const AVAHI_DOMAIN: &str = "";
const AVAHI_UNSPEC: i32 = -1;
const AVAHI_LOOKUP_FLAGS_NONE: u32 = 0;
const CLEANUP_RESERVE: Duration = Duration::from_millis(250);
const MAX_RUNTIME_EVENTS: usize = 256;
const MAX_SIGNAL_QUEUE: usize = 64;

/// Closed failures for the Linux runtime adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvahiDiscoveryError {
    InvalidTimeout,
    SystemBusUnavailable,
    AvahiUnavailable,
    SignalSetupFailed,
    BrowserCreateFailed,
    ProtocolViolation,
    EventLimitExceeded,
    CandidateLimitExceeded,
    BrowserReleaseFailed,
}

impl fmt::Display for AvahiDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTimeout => "Avahi discovery timeout must be 1 through 30 seconds",
            Self::SystemBusUnavailable => "the system D-Bus is unavailable",
            Self::AvahiUnavailable => "Avahi is unavailable",
            Self::SignalSetupFailed => "Avahi browser signal setup failed",
            Self::BrowserCreateFailed => "Avahi service browser creation failed",
            Self::ProtocolViolation => "Avahi returned an invalid browser message",
            Self::EventLimitExceeded => "Avahi discovery event limit was exceeded",
            Self::CandidateLimitExceeded => "Avahi discovery candidate limit was exceeded",
            Self::BrowserReleaseFailed => "Avahi service browser release failed",
        })
    }
}

impl std::error::Error for AvahiDiscoveryError {}

/// Runs one read-only Avahi scan and returns only the sanitized summary.
///
/// The function creates a private system-bus connection, installs the signal
/// match before creating the browser, and explicitly calls `Free` before the
/// connection is dropped. It never chooses a candidate.
pub fn discover_avahi_summary(timeout: Duration) -> Result<DiscoverySummary, AvahiDiscoveryError> {
    let options = DiscoveryOptions::new(timeout).map_err(map_options_error)?;
    let started = Instant::now();
    let overall_deadline = started
        .checked_add(options.timeout())
        .ok_or(AvahiDiscoveryError::InvalidTimeout)?;
    let work_deadline = overall_deadline
        .checked_sub(CLEANUP_RESERVE)
        .ok_or(AvahiDiscoveryError::InvalidTimeout)?;
    let mut browser = AvahiBrowser::new(work_deadline)?;
    collect_and_release(&mut browser, work_deadline, overall_deadline)
}

fn map_options_error(error: DiscoveryError) -> AvahiDiscoveryError {
    match error {
        DiscoveryError::InvalidTimeout => AvahiDiscoveryError::InvalidTimeout,
        DiscoveryError::EventLimitExceeded => AvahiDiscoveryError::EventLimitExceeded,
        DiscoveryError::CandidateLimitExceeded => AvahiDiscoveryError::CandidateLimitExceeded,
        DiscoveryError::SourceUnavailable => AvahiDiscoveryError::AvahiUnavailable,
    }
}

fn remaining(deadline: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then_some(remaining)
}

struct BrowserItem {
    interface: i32,
    protocol: i32,
    name: String,
    service_type: String,
    domain: String,
}

enum BrowserSignal {
    ItemNew(BrowserItem),
    NoRecord,
    AllForNow,
    Failure,
    Invalid,
}

fn decode_browser_signal(message: &Message) -> BrowserSignal {
    match message.member().as_ref().map(|member| member.as_ref()) {
        Some("ItemNew") => {
            let mut arguments = message.iter_init();
            let decoded = (|| {
                let interface = arguments.read::<i32>().ok()?;
                let protocol = arguments.read::<i32>().ok()?;
                let name = arguments.read::<String>().ok()?;
                let service_type = arguments.read::<String>().ok()?;
                let domain = arguments.read::<String>().ok()?;
                let _flags = arguments.read::<u32>().ok()?;
                Some(BrowserItem {
                    interface,
                    protocol,
                    name,
                    service_type,
                    domain,
                })
            })();
            decoded.map_or(BrowserSignal::Invalid, BrowserSignal::ItemNew)
        }
        Some("ItemRemove") | Some("CacheExhausted") => BrowserSignal::NoRecord,
        Some("AllForNow") => BrowserSignal::AllForNow,
        Some("Failure") => BrowserSignal::Failure,
        _ => BrowserSignal::Invalid,
    }
}

enum RuntimeEvent {
    Record(ResolvedMdnsRecord),
    NoRecord,
    AllForNow,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeFailure {
    Unavailable,
    ProtocolViolation,
    ReleaseFailed,
}

trait BrowserRuntime {
    fn next_event(&mut self, maximum_wait: Duration) -> Result<RuntimeEvent, RuntimeFailure>;
    fn release(&mut self, maximum_wait: Duration) -> Result<(), RuntimeFailure>;
}

fn collect_and_release(
    runtime: &mut impl BrowserRuntime,
    work_deadline: Instant,
    overall_deadline: Instant,
) -> Result<DiscoverySummary, AvahiDiscoveryError> {
    let scan_result = collect_runtime(runtime, work_deadline);
    let release_result = runtime.release(remaining(overall_deadline).unwrap_or(Duration::ZERO));
    if release_result.is_err() {
        return Err(AvahiDiscoveryError::BrowserReleaseFailed);
    }
    scan_result
}

fn collect_runtime(
    runtime: &mut impl BrowserRuntime,
    work_deadline: Instant,
) -> Result<DiscoverySummary, AvahiDiscoveryError> {
    let mut collector = CandidateCollector::new();
    let mut event_count = 0_usize;
    loop {
        let Some(maximum_wait) = remaining(work_deadline) else {
            return Ok(collector.finish(true));
        };
        event_count += 1;
        if event_count > MAX_RUNTIME_EVENTS {
            return Err(AvahiDiscoveryError::EventLimitExceeded);
        }
        match runtime
            .next_event(maximum_wait)
            .map_err(map_runtime_failure)?
        {
            RuntimeEvent::Record(record) => collector.observe(record).map_err(map_options_error)?,
            RuntimeEvent::NoRecord => {}
            RuntimeEvent::AllForNow => return Ok(collector.finish(false)),
            RuntimeEvent::TimedOut => return Ok(collector.finish(true)),
        }
    }
}

fn map_runtime_failure(failure: RuntimeFailure) -> AvahiDiscoveryError {
    match failure {
        RuntimeFailure::Unavailable => AvahiDiscoveryError::AvahiUnavailable,
        RuntimeFailure::ProtocolViolation => AvahiDiscoveryError::ProtocolViolation,
        RuntimeFailure::ReleaseFailed => AvahiDiscoveryError::BrowserReleaseFailed,
    }
}

struct AvahiBrowser {
    connection: Connection,
    browser_path: Path<'static>,
    signal_token: Token,
    queue: Arc<Mutex<VecDeque<BrowserSignal>>>,
    released: bool,
}

impl AvahiBrowser {
    fn new(deadline: Instant) -> Result<Self, AvahiDiscoveryError> {
        let connection =
            Connection::new_system().map_err(|_| AvahiDiscoveryError::SystemBusUnavailable)?;

        let owner_timeout = remaining(deadline).ok_or(AvahiDiscoveryError::AvahiUnavailable)?;
        let owner_proxy = connection.with_proxy(DBUS_BUS, DBUS_PATH, owner_timeout);
        let (owner,): (String,) = owner_proxy
            .method_call(DBUS_INTERFACE, "GetNameOwner", (AVAHI_BUS,))
            .map_err(|_| AvahiDiscoveryError::AvahiUnavailable)?;

        let rule = MatchRule::new()
            .with_type(MessageType::Signal)
            .with_interface(AVAHI_BROWSER_INTERFACE)
            .with_strict_sender(owner)
            .static_clone();
        let match_text = rule.match_str();
        let match_timeout = remaining(deadline).ok_or(AvahiDiscoveryError::SignalSetupFailed)?;
        let match_proxy = connection.with_proxy(DBUS_BUS, DBUS_PATH, match_timeout);
        let match_result: Result<(), dbus::Error> =
            match_proxy.method_call(DBUS_INTERFACE, "AddMatch", (match_text.as_str(),));
        match_result.map_err(|_| AvahiDiscoveryError::SignalSetupFailed)?;

        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let expected_path = Arc::new(Mutex::new(None::<String>));
        let callback_queue = Arc::clone(&queue);
        let callback_path = Arc::clone(&expected_path);
        let signal_token = connection.start_receive(
            rule,
            Box::new(move |message, _| {
                let path_matches = callback_path.lock().ok().is_some_and(|expected_path| {
                    expected_path.as_ref().is_some_and(|expected| {
                        message
                            .path()
                            .as_ref()
                            .is_some_and(|path| path.to_string() == *expected)
                    })
                });
                if path_matches {
                    if let Ok(mut queue) = callback_queue.lock() {
                        if queue.len() >= MAX_SIGNAL_QUEUE {
                            queue.clear();
                            queue.push_back(BrowserSignal::Invalid);
                        } else {
                            queue.push_back(decode_browser_signal(&message));
                        }
                    }
                }
                true
            }),
        );

        let browser_timeout =
            remaining(deadline).ok_or(AvahiDiscoveryError::BrowserCreateFailed)?;
        let server = connection.with_proxy(AVAHI_BUS, AVAHI_SERVER_PATH, browser_timeout);
        let created: Result<(Path<'static>,), dbus::Error> = server.method_call(
            AVAHI_SERVER_INTERFACE,
            "ServiceBrowserNew",
            (
                AVAHI_UNSPEC,
                AVAHI_UNSPEC,
                AVAHI_SERVICE_TYPE,
                AVAHI_DOMAIN,
                AVAHI_LOOKUP_FLAGS_NONE,
            ),
        );
        let (browser_path,) = match created {
            Ok(created) => created,
            Err(_) => {
                let _ = connection.stop_receive(signal_token);
                return Err(AvahiDiscoveryError::BrowserCreateFailed);
            }
        };
        *expected_path
            .lock()
            .map_err(|_| AvahiDiscoveryError::SignalSetupFailed)? = Some(browser_path.to_string());

        Ok(Self {
            connection,
            browser_path,
            signal_token,
            queue,
            released: false,
        })
    }

    fn resolve_item(
        &self,
        item: BrowserItem,
        timeout: Duration,
    ) -> Result<Option<ResolvedMdnsRecord>, RuntimeFailure> {
        if item.interface < 0
            || !matches!(item.protocol, 0 | 1)
            || item.name.is_empty()
            || item.name.len() > 255
            || !item.name.is_ascii()
            || !item.service_type.eq_ignore_ascii_case(AVAHI_SERVICE_TYPE)
            || item.domain.is_empty()
            || item.domain.len() > 255
            || !item.domain.is_ascii()
        {
            return Err(RuntimeFailure::ProtocolViolation);
        }

        type ResolveReply = (
            i32,
            i32,
            String,
            String,
            String,
            String,
            i32,
            String,
            u16,
            Vec<Vec<u8>>,
            u32,
        );
        let server = self
            .connection
            .with_proxy(AVAHI_BUS, AVAHI_SERVER_PATH, timeout);
        let resolved: ResolveReply = match server.method_call(
            AVAHI_SERVER_INTERFACE,
            "ResolveService",
            (
                item.interface,
                item.protocol,
                item.name.as_str(),
                item.service_type.as_str(),
                item.domain.as_str(),
                AVAHI_UNSPEC,
                AVAHI_LOOKUP_FLAGS_NONE,
            ),
        ) {
            Ok(resolved) => resolved,
            // A service can disappear between ItemNew and ResolveService.
            Err(_) => return Ok(None),
        };
        let (
            resolved_interface,
            resolved_protocol,
            resolved_name,
            resolved_type,
            resolved_domain,
            _host,
            address_protocol,
            address,
            port,
            txt,
            _flags,
        ) = resolved;
        if resolved_interface != item.interface
            || resolved_protocol != item.protocol
            || resolved_name != item.name
            || !resolved_type.eq_ignore_ascii_case(&item.service_type)
            || !resolved_domain.eq_ignore_ascii_case(&item.domain)
            || !matches!(address_protocol, 0 | 1)
            || port == 0
        {
            return Err(RuntimeFailure::ProtocolViolation);
        }
        let Ok(address) = address.parse::<IpAddr>() else {
            return Ok(None);
        };
        let Some(txt) = split_txt_entries(txt) else {
            return Ok(None);
        };
        Ok(ResolvedMdnsRecord::new(JBL_MDNS_SERVICE_TYPE, &item.name, [address], txt).ok())
    }

    fn send_free_no_reply(&self) {
        let Ok(mut message) = Message::new_method_call(
            AVAHI_BUS,
            self.browser_path.clone(),
            AVAHI_BROWSER_INTERFACE,
            "Free",
        ) else {
            return;
        };
        message.set_no_reply(true);
        let _ = self.connection.send(message);
    }
}

impl BrowserRuntime for AvahiBrowser {
    fn next_event(&mut self, maximum_wait: Duration) -> Result<RuntimeEvent, RuntimeFailure> {
        let deadline = Instant::now()
            .checked_add(maximum_wait)
            .ok_or(RuntimeFailure::Unavailable)?;
        loop {
            let signal = self
                .queue
                .lock()
                .map_err(|_| RuntimeFailure::Unavailable)?
                .pop_front();
            if let Some(signal) = signal {
                return match signal {
                    BrowserSignal::ItemNew(item) => {
                        let Some(resolve_timeout) = remaining(deadline) else {
                            return Ok(RuntimeEvent::TimedOut);
                        };
                        self.resolve_item(item, resolve_timeout).map(|record| {
                            record.map_or(RuntimeEvent::NoRecord, RuntimeEvent::Record)
                        })
                    }
                    BrowserSignal::NoRecord => Ok(RuntimeEvent::NoRecord),
                    BrowserSignal::AllForNow => Ok(RuntimeEvent::AllForNow),
                    BrowserSignal::Failure => Err(RuntimeFailure::Unavailable),
                    BrowserSignal::Invalid => Err(RuntimeFailure::ProtocolViolation),
                };
            }
            let Some(process_timeout) = remaining(deadline) else {
                return Ok(RuntimeEvent::TimedOut);
            };
            let processed = self
                .connection
                .process(process_timeout)
                .map_err(|_| RuntimeFailure::Unavailable)?;
            if !processed {
                return Ok(RuntimeEvent::TimedOut);
            }
        }
    }

    fn release(&mut self, maximum_wait: Duration) -> Result<(), RuntimeFailure> {
        if self.released {
            return Ok(());
        }
        if maximum_wait.is_zero() {
            self.send_free_no_reply();
            return Err(RuntimeFailure::ReleaseFailed);
        }
        let browser =
            self.connection
                .with_proxy(AVAHI_BUS, self.browser_path.clone(), maximum_wait);
        let released: Result<(), dbus::Error> =
            browser.method_call(AVAHI_BROWSER_INTERFACE, "Free", ());
        released.map_err(|_| RuntimeFailure::ReleaseFailed)?;
        self.released = true;
        let _ = self.connection.stop_receive(self.signal_token);
        Ok(())
    }
}

impl Drop for AvahiBrowser {
    fn drop(&mut self) {
        if !self.released {
            self.send_free_no_reply();
        }
        let _ = self.connection.stop_receive(self.signal_token);
    }
}

fn split_txt_entries(entries: Vec<Vec<u8>>) -> Option<Vec<(String, Vec<u8>)>> {
    let mut split = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.len() > 255 {
            return None;
        }
        let separator = entry.iter().position(|byte| *byte == b'=');
        let (raw_key, value) = match separator {
            Some(index) => (&entry[..index], entry[index + 1..].to_vec()),
            None => (entry.as_slice(), Vec::new()),
        };
        let key = std::str::from_utf8(raw_key).ok()?.to_string();
        split.push((key, value));
    }
    Some(split)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    struct FakeBrowser {
        events: VecDeque<Result<RuntimeEvent, RuntimeFailure>>,
        waits: Vec<Duration>,
        releases: usize,
        release_result: Result<(), RuntimeFailure>,
    }

    impl FakeBrowser {
        fn new(events: impl IntoIterator<Item = RuntimeEvent>) -> Self {
            Self {
                events: events.into_iter().map(Ok).collect(),
                waits: Vec::new(),
                releases: 0,
                release_result: Ok(()),
            }
        }
    }

    impl BrowserRuntime for FakeBrowser {
        fn next_event(&mut self, maximum_wait: Duration) -> Result<RuntimeEvent, RuntimeFailure> {
            self.waits.push(maximum_wait);
            self.events
                .pop_front()
                .unwrap_or(Ok(RuntimeEvent::AllForNow))
        }

        fn release(&mut self, maximum_wait: Duration) -> Result<(), RuntimeFailure> {
            assert!(!maximum_wait.is_zero());
            self.releases += 1;
            self.release_result
        }
    }

    fn record(name: &str, address: IpAddr) -> ResolvedMdnsRecord {
        ResolvedMdnsRecord::new(
            JBL_MDNS_SERVICE_TYPE,
            name,
            [address],
            [("model", b"private-model".as_slice())],
        )
        .unwrap()
    }

    fn deadlines() -> (Instant, Instant) {
        let now = Instant::now();
        (now + Duration::from_secs(1), now + Duration::from_secs(2))
    }

    #[test]
    fn fake_runtime_preserves_multiple_candidates_and_releases_once() {
        let mut runtime = FakeBrowser::new([
            RuntimeEvent::Record(record("one", IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)))),
            RuntimeEvent::Record(record(
                "two",
                IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 2)),
            )),
            RuntimeEvent::AllForNow,
        ]);
        let (work, overall) = deadlines();
        let summary = collect_and_release(&mut runtime, work, overall).unwrap();
        assert_eq!(summary.candidate_count, 2);
        assert!(!summary.timed_out);
        assert_eq!(runtime.releases, 1);
        assert!(runtime.waits.iter().all(|wait| !wait.is_zero()));
    }

    #[test]
    fn fake_timeout_returns_empty_summary_and_still_releases() {
        let mut runtime = FakeBrowser::new([RuntimeEvent::TimedOut]);
        let (work, overall) = deadlines();
        let summary = collect_and_release(&mut runtime, work, overall).unwrap();
        assert_eq!(summary.candidate_count, 0);
        assert!(summary.timed_out);
        assert_eq!(runtime.releases, 1);
    }

    #[test]
    fn scan_failure_and_release_failure_both_release_and_fail_closed() {
        let mut scan_failure = FakeBrowser {
            events: VecDeque::from([Err(RuntimeFailure::ProtocolViolation)]),
            waits: Vec::new(),
            releases: 0,
            release_result: Ok(()),
        };
        let (work, overall) = deadlines();
        assert_eq!(
            collect_and_release(&mut scan_failure, work, overall),
            Err(AvahiDiscoveryError::ProtocolViolation)
        );
        assert_eq!(scan_failure.releases, 1);

        let mut release_failure = FakeBrowser::new([RuntimeEvent::AllForNow]);
        release_failure.release_result = Err(RuntimeFailure::ReleaseFailed);
        let (work, overall) = deadlines();
        assert_eq!(
            collect_and_release(&mut release_failure, work, overall),
            Err(AvahiDiscoveryError::BrowserReleaseFailed)
        );
        assert_eq!(release_failure.releases, 1);
    }

    #[test]
    fn txt_split_is_bounded_and_never_interprets_values() {
        assert_eq!(
            split_txt_entries(vec![b"model=private".to_vec(), b"flag".to_vec()]),
            Some(vec![
                ("model".to_string(), b"private".to_vec()),
                ("flag".to_string(), Vec::new()),
            ])
        );
        assert!(split_txt_entries(vec![vec![b'x'; 256]]).is_none());
        assert!(split_txt_entries(vec![vec![0xff, b'=', b'x']]).is_none());
    }
}
