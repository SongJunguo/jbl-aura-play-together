//! Dependency-neutral, privacy-safe JBL mDNS candidate discovery core.
//!
//! A platform adapter may later translate resolved mDNS records into
//! [`ResolvedMdnsRecord`]. This module deliberately owns no socket and emits no
//! IP address, service instance, TXT value or device identifier.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const JBL_MDNS_SERVICE_TYPE: &str = "_jbl-product._tcp.local.";
const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const MIN_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DISCOVERY_EVENTS: usize = 256;
const MAX_CANDIDATES: usize = 64;
const MAX_ADDRESSES_PER_RECORD: usize = 16;
const MAX_TXT_ENTRIES: usize = 64;
const MAX_TXT_ENTRY_BYTES: usize = 255;
const MAX_TXT_KEY_BYTES: usize = 63;
const MAX_INSTANCE_BYTES: usize = 255;

/// Presence-only projection of the fixed TXT allowlist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TxtFieldPresence {
    pub has_fn: bool,
    pub has_name: bool,
    pub has_id: bool,
    pub has_uuid: bool,
    pub has_md: bool,
    pub has_model: bool,
}

impl TxtFieldPresence {
    fn merge(&mut self, other: Self) {
        self.has_fn |= other.has_fn;
        self.has_name |= other.has_name;
        self.has_id |= other.has_id;
        self.has_uuid |= other.has_uuid;
        self.has_md |= other.has_md;
        self.has_model |= other.has_model;
    }
}

/// Sanitized projection of one unique service instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SanitizedCandidate {
    pub has_ipv4: bool,
    pub has_ipv6: bool,
    pub txt: TxtFieldPresence,
}

/// Candidate cardinality without an implicit selection policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateCardinality {
    None,
    One,
    Multiple,
}

/// Complete output of one bounded discovery window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoverySummary {
    pub candidate_count: usize,
    pub cardinality: CandidateCardinality,
    pub candidates: Vec<SanitizedCandidate>,
    pub timed_out: bool,
}

/// Closed validation failures for one resolved mDNS record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    WrongServiceType,
    InvalidInstance,
    TooManyAddresses,
    NoUsableAddress,
    TooManyTxtEntries,
    InvalidTxtEntry,
    DuplicateTxtKey,
}

impl fmt::Display for RecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongServiceType => "resolved record used an unexpected service type",
            Self::InvalidInstance => "resolved record contained an invalid service instance",
            Self::TooManyAddresses => "resolved record contained too many addresses",
            Self::NoUsableAddress => "resolved record contained no usable address",
            Self::TooManyTxtEntries => "resolved record contained too many TXT entries",
            Self::InvalidTxtEntry => "resolved record contained an invalid TXT entry",
            Self::DuplicateTxtKey => "resolved record contained a duplicate TXT key",
        })
    }
}

impl std::error::Error for RecordError {}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateKey([u8; 32]);

/// Validated input record. It intentionally implements neither `Debug` nor
/// `Serialize`, and retains no raw instance, TXT key/value or address.
pub struct ResolvedMdnsRecord {
    key: CandidateKey,
    candidate: SanitizedCandidate,
}

impl ResolvedMdnsRecord {
    /// Validates and immediately reduces one resolved service record.
    pub fn new<A, T, K, V>(
        service_type: &str,
        service_instance: &str,
        addresses: A,
        txt_entries: T,
    ) -> Result<Self, RecordError>
    where
        A: IntoIterator<Item = IpAddr>,
        T: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<[u8]>,
    {
        if !service_type.eq_ignore_ascii_case(JBL_MDNS_SERVICE_TYPE) {
            return Err(RecordError::WrongServiceType);
        }
        if service_instance.is_empty()
            || service_instance.len() > MAX_INSTANCE_BYTES
            || !service_instance.is_ascii()
        {
            return Err(RecordError::InvalidInstance);
        }

        let mut has_ipv4 = false;
        let mut has_ipv6 = false;
        let mut address_count = 0_usize;
        for address in addresses {
            address_count += 1;
            if address_count > MAX_ADDRESSES_PER_RECORD {
                return Err(RecordError::TooManyAddresses);
            }
            if usable_address(address) {
                match address {
                    IpAddr::V4(_) => has_ipv4 = true,
                    IpAddr::V6(_) => has_ipv6 = true,
                }
            }
        }
        if !has_ipv4 && !has_ipv6 {
            return Err(RecordError::NoUsableAddress);
        }

        let mut txt = TxtFieldPresence::default();
        let mut seen_keys = BTreeSet::new();
        let mut txt_count = 0_usize;
        for (raw_key, raw_value) in txt_entries {
            txt_count += 1;
            if txt_count > MAX_TXT_ENTRIES {
                return Err(RecordError::TooManyTxtEntries);
            }
            let key = raw_key.as_ref();
            let value = raw_value.as_ref();
            if !valid_txt_key(key) || key.len() + 1 + value.len() > MAX_TXT_ENTRY_BYTES {
                return Err(RecordError::InvalidTxtEntry);
            }
            let normalized = key.to_ascii_lowercase();
            if !seen_keys.insert(normalized.clone()) {
                return Err(RecordError::DuplicateTxtKey);
            }
            if value.is_empty() {
                continue;
            }
            match normalized.as_str() {
                "fn" => txt.has_fn = true,
                "name" => txt.has_name = true,
                "id" => txt.has_id = true,
                "uuid" => txt.has_uuid = true,
                "md" => txt.has_md = true,
                "model" => txt.has_model = true,
                _ => {}
            }
        }

        let mut hasher = Sha256::new();
        hasher.update(JBL_MDNS_SERVICE_TYPE.as_bytes());
        hasher.update([0]);
        for byte in service_instance.bytes() {
            hasher.update([byte.to_ascii_lowercase()]);
        }
        let key = CandidateKey(hasher.finalize().into());
        Ok(Self {
            key,
            candidate: SanitizedCandidate {
                has_ipv4,
                has_ipv6,
                txt,
            },
        })
    }
}

fn valid_txt_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_TXT_KEY_BYTES
        && key
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte) && byte != b'=')
}

fn usable_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !address.is_broadcast()
        }
        IpAddr::V6(address) => {
            !address.is_unspecified() && !address.is_loopback() && !address.is_multicast()
        }
    }
}

#[derive(Clone, Copy)]
struct CandidateAggregate(SanitizedCandidate);

impl CandidateAggregate {
    fn merge(&mut self, candidate: SanitizedCandidate) {
        self.0.has_ipv4 |= candidate.has_ipv4;
        self.0.has_ipv6 |= candidate.has_ipv6;
        self.0.txt.merge(candidate.txt);
    }
}

/// Internal collector with no API that chooses or returns a first candidate.
pub struct CandidateCollector {
    candidates: BTreeMap<CandidateKey, CandidateAggregate>,
}

impl CandidateCollector {
    pub const fn new() -> Self {
        Self {
            candidates: BTreeMap::new(),
        }
    }

    pub fn observe(&mut self, record: ResolvedMdnsRecord) -> Result<(), DiscoveryError> {
        if let Some(existing) = self.candidates.get_mut(&record.key) {
            existing.merge(record.candidate);
            return Ok(());
        }
        if self.candidates.len() >= MAX_CANDIDATES {
            return Err(DiscoveryError::CandidateLimitExceeded);
        }
        self.candidates
            .insert(record.key, CandidateAggregate(record.candidate));
        Ok(())
    }

    pub fn finish(self, timed_out: bool) -> DiscoverySummary {
        let candidates = self
            .candidates
            .into_values()
            .map(|candidate| candidate.0)
            .collect::<Vec<_>>();
        let candidate_count = candidates.len();
        let cardinality = match candidate_count {
            0 => CandidateCardinality::None,
            1 => CandidateCardinality::One,
            _ => CandidateCardinality::Multiple,
        };
        DiscoverySummary {
            candidate_count,
            cardinality,
            candidates,
            timed_out,
        }
    }
}

impl Default for CandidateCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFailure {
    Unavailable,
}

/// One already-sanitizable result from a future platform mDNS adapter.
pub enum DiscoveryPoll {
    Resolved(ResolvedMdnsRecord),
    NoRecord,
    TimedOut,
    Closed,
}

/// Dependency-neutral boundary for a future platform mDNS implementation.
pub trait DiscoverySource {
    /// Implementations must not wait longer than `maximum_wait`.
    fn poll(&mut self, maximum_wait: Duration) -> Result<DiscoveryPoll, SourceFailure>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryError {
    InvalidTimeout,
    SourceUnavailable,
    EventLimitExceeded,
    CandidateLimitExceeded,
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTimeout => "mDNS discovery timeout is invalid",
            Self::SourceUnavailable => "mDNS discovery source is unavailable",
            Self::EventLimitExceeded => "mDNS discovery event limit was exceeded",
            Self::CandidateLimitExceeded => "mDNS candidate limit was exceeded",
        })
    }
}

impl std::error::Error for DiscoveryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryOptions {
    timeout: Duration,
}

impl DiscoveryOptions {
    pub fn new(timeout: Duration) -> Result<Self, DiscoveryError> {
        if timeout < MIN_DISCOVERY_TIMEOUT || timeout > MAX_DISCOVERY_TIMEOUT {
            return Err(DiscoveryError::InvalidTimeout);
        }
        Ok(Self { timeout })
    }

    pub const fn timeout(self) -> Duration {
        self.timeout
    }
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_DISCOVERY_TIMEOUT,
        }
    }
}

/// Runs one bounded collection window over an injected event source.
pub fn discover_bounded(
    source: &mut impl DiscoverySource,
    options: DiscoveryOptions,
) -> Result<DiscoverySummary, DiscoveryError> {
    let started = Instant::now();
    let deadline = started
        .checked_add(options.timeout())
        .ok_or(DiscoveryError::InvalidTimeout)?;
    let mut collector = CandidateCollector::new();
    let mut event_count = 0_usize;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(collector.finish(true));
        }
        let remaining = deadline.saturating_duration_since(now);
        let event = source
            .poll(remaining)
            .map_err(|SourceFailure::Unavailable| DiscoveryError::SourceUnavailable)?;
        event_count += 1;
        if event_count > MAX_DISCOVERY_EVENTS {
            return Err(DiscoveryError::EventLimitExceeded);
        }
        match event {
            DiscoveryPoll::Resolved(record) => collector.observe(record)?,
            DiscoveryPoll::NoRecord => {}
            DiscoveryPoll::TimedOut => return Ok(collector.finish(true)),
            DiscoveryPoll::Closed => return Ok(collector.finish(false)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn ipv4(index: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, index))
    }

    fn ipv6(index: u16) -> IpAddr {
        IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, index))
    }

    fn record(
        instance: &str,
        addresses: Vec<IpAddr>,
        txt: Vec<(&str, &[u8])>,
    ) -> ResolvedMdnsRecord {
        ResolvedMdnsRecord::new(JBL_MDNS_SERVICE_TYPE, instance, addresses, txt).unwrap()
    }

    struct FakeSource {
        events: VecDeque<Result<DiscoveryPoll, SourceFailure>>,
        waits: Vec<Duration>,
    }

    impl FakeSource {
        fn new(events: impl IntoIterator<Item = DiscoveryPoll>) -> Self {
            Self {
                events: events.into_iter().map(Ok).collect(),
                waits: Vec::new(),
            }
        }
    }

    impl DiscoverySource for FakeSource {
        fn poll(&mut self, maximum_wait: Duration) -> Result<DiscoveryPoll, SourceFailure> {
            self.waits.push(maximum_wait);
            self.events.pop_front().unwrap_or(Ok(DiscoveryPoll::Closed))
        }
    }

    #[test]
    fn zero_one_and_multiple_candidates_remain_distinct_without_selection() {
        let empty = CandidateCollector::new().finish(false);
        assert_eq!(empty.candidate_count, 0);
        assert_eq!(empty.cardinality, CandidateCardinality::None);

        let mut one = CandidateCollector::new();
        one.observe(record("one", vec![ipv4(1)], vec![])).unwrap();
        let one = one.finish(false);
        assert_eq!(one.candidate_count, 1);
        assert_eq!(one.cardinality, CandidateCardinality::One);

        let mut multiple = CandidateCollector::new();
        multiple
            .observe(record("one", vec![ipv4(1)], vec![]))
            .unwrap();
        multiple
            .observe(record("two", vec![ipv4(2)], vec![]))
            .unwrap();
        let multiple = multiple.finish(false);
        assert_eq!(multiple.candidate_count, 2);
        assert_eq!(multiple.cardinality, CandidateCardinality::Multiple);
    }

    #[test]
    fn duplicate_instance_records_merge_families_and_allowlisted_presence() {
        let mut collector = CandidateCollector::new();
        collector
            .observe(record(
                "same-instance",
                vec![ipv4(1)],
                vec![("fn", b"speaker")],
            ))
            .unwrap();
        collector
            .observe(record(
                "SAME-INSTANCE",
                vec![ipv6(1)],
                vec![("model", b"model")],
            ))
            .unwrap();
        let summary = collector.finish(false);
        assert_eq!(summary.candidate_count, 1);
        assert!(summary.candidates[0].has_ipv4);
        assert!(summary.candidates[0].has_ipv6);
        assert!(summary.candidates[0].txt.has_fn);
        assert!(summary.candidates[0].txt.has_model);
    }

    #[test]
    fn txt_projection_is_fixed_presence_only_and_never_echoes_values() {
        let private_value = b"private-value".as_slice();
        let candidate = record(
            "private-instance",
            vec![ipv4(1)],
            vec![
                ("fn", private_value),
                ("name", private_value),
                ("id", private_value),
                ("uuid", private_value),
                ("md", private_value),
                ("model", private_value),
                ("unknown", private_value),
            ],
        );
        let mut collector = CandidateCollector::new();
        collector.observe(candidate).unwrap();
        let serialized = serde_json::to_string(&collector.finish(false)).unwrap();
        assert!(!serialized.contains(std::str::from_utf8(private_value).unwrap()));
        assert!(!serialized.contains("private-instance"));
        assert!(!serialized.contains(&ipv4(1).to_string()));
        for field in [
            "has_fn",
            "has_name",
            "has_id",
            "has_uuid",
            "has_md",
            "has_model",
        ] {
            assert!(serialized.contains(field));
        }
        assert!(!serialized.contains("unknown"));
    }

    #[test]
    fn empty_allowlisted_values_are_not_reported_as_present() {
        let candidate = record(
            "empty-values",
            vec![ipv4(1)],
            vec![("fn", b""), ("uuid", b"")],
        );
        assert_eq!(candidate.candidate.txt, TxtFieldPresence::default());
    }

    #[test]
    fn malicious_duplicate_or_overlong_txt_is_rejected() {
        assert_eq!(
            ResolvedMdnsRecord::new(
                JBL_MDNS_SERVICE_TYPE,
                "duplicate",
                [ipv4(1)],
                [("UUID", b"one".as_slice()), ("uuid", b"two".as_slice())],
            )
            .err(),
            Some(RecordError::DuplicateTxtKey)
        );
        assert_eq!(
            ResolvedMdnsRecord::new(
                JBL_MDNS_SERVICE_TYPE,
                "control-key",
                [ipv4(1)],
                [("bad\nkey", b"value".as_slice())],
            )
            .err(),
            Some(RecordError::InvalidTxtEntry)
        );
        let oversized = vec![b'x'; MAX_TXT_ENTRY_BYTES];
        assert_eq!(
            ResolvedMdnsRecord::new(
                JBL_MDNS_SERVICE_TYPE,
                "oversized",
                [ipv4(1)],
                [("uuid", oversized)],
            )
            .err(),
            Some(RecordError::InvalidTxtEntry)
        );
    }

    #[test]
    fn wrong_service_overlong_instance_and_invalid_addresses_fail_closed() {
        assert_eq!(
            ResolvedMdnsRecord::new(
                "_spotify-connect._tcp.local.",
                "other-service",
                [ipv4(1)],
                Vec::<(&str, &[u8])>::new(),
            )
            .err(),
            Some(RecordError::WrongServiceType)
        );
        let long_instance = "x".repeat(MAX_INSTANCE_BYTES + 1);
        assert_eq!(
            ResolvedMdnsRecord::new(
                JBL_MDNS_SERVICE_TYPE,
                &long_instance,
                [ipv4(1)],
                Vec::<(&str, &[u8])>::new(),
            )
            .err(),
            Some(RecordError::InvalidInstance)
        );
        assert_eq!(
            ResolvedMdnsRecord::new(
                JBL_MDNS_SERVICE_TYPE,
                "invalid-address",
                [
                    IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    IpAddr::V6(Ipv6Addr::LOCALHOST)
                ],
                Vec::<(&str, &[u8])>::new(),
            )
            .err(),
            Some(RecordError::NoUsableAddress)
        );
    }

    #[test]
    fn bounded_source_preserves_candidates_and_reports_timeout() {
        let mut source = FakeSource::new([
            DiscoveryPoll::Resolved(record("one", vec![ipv4(1)], vec![("md", b"model")])),
            DiscoveryPoll::Resolved(record("two", vec![ipv6(2)], vec![("uuid", b"id")])),
            DiscoveryPoll::TimedOut,
        ]);
        let options = DiscoveryOptions::new(Duration::from_secs(2)).unwrap();
        let summary = discover_bounded(&mut source, options).unwrap();
        assert!(summary.timed_out);
        assert_eq!(summary.cardinality, CandidateCardinality::Multiple);
        assert_eq!(summary.candidate_count, 2);
        assert!(source
            .waits
            .iter()
            .all(|wait| !wait.is_zero() && *wait <= options.timeout()));
    }

    #[test]
    fn timeout_without_records_preserves_an_empty_result() {
        let mut source = FakeSource::new([DiscoveryPoll::TimedOut]);
        let summary = discover_bounded(&mut source, DiscoveryOptions::default()).unwrap();
        assert!(summary.timed_out);
        assert_eq!(summary.candidate_count, 0);
        assert_eq!(summary.cardinality, CandidateCardinality::None);
    }

    #[test]
    fn discovery_timeout_is_nonzero_and_capped() {
        assert_eq!(
            DiscoveryOptions::new(Duration::ZERO),
            Err(DiscoveryError::InvalidTimeout)
        );
        assert_eq!(
            DiscoveryOptions::new(MIN_DISCOVERY_TIMEOUT - Duration::from_nanos(1)),
            Err(DiscoveryError::InvalidTimeout)
        );
        assert_eq!(
            DiscoveryOptions::new(MAX_DISCOVERY_TIMEOUT + Duration::from_nanos(1)),
            Err(DiscoveryError::InvalidTimeout)
        );
        assert_eq!(
            DiscoveryOptions::new(MAX_DISCOVERY_TIMEOUT)
                .unwrap()
                .timeout(),
            MAX_DISCOVERY_TIMEOUT
        );
    }

    #[test]
    fn source_failure_is_closed_and_contains_no_diagnostic() {
        let mut source = FakeSource {
            events: VecDeque::from([Err(SourceFailure::Unavailable)]),
            waits: Vec::new(),
        };
        assert_eq!(
            discover_bounded(&mut source, DiscoveryOptions::default()),
            Err(DiscoveryError::SourceUnavailable)
        );
    }
}
