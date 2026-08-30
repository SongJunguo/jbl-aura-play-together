//! Native Linux Aura Studio 5 control over BlueZ.
//!
//! The public surface is deliberately closed: it exposes only Play Together
//! on/off, coarse health, and sanitized failures.  Rotating LE addresses,
//! advertisement payloads, D-Bus diagnostics, object paths and GATT values are
//! never formatted or returned.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::os::fd::{AsRawFd, RawFd};
use std::pin::Pin;
use std::time::Duration;

use bluer::l2cap::{
    Security, SecurityLevel, SeqPacket, Socket as L2capSocket, SocketAddr as L2capSocketAddr,
};
use bluer::{
    Adapter, AdapterEvent, Address, AddressType, Device, DeviceEvent, DeviceProperty,
    DiscoveryFilter, DiscoveryTransport, Session, Uuid,
};
use dbus::arg::PropMap;
use dbus::blocking::stdintf::org_freedesktop_dbus::ObjectManager;
use dbus::blocking::Connection as BlockingDbusConnection;
use futures::{stream::SelectAll, FutureExt, Stream, StreamExt};
use tokio::runtime::{Builder, Runtime};
use tokio::time::{self, Instant};

use crate::aura_protocol::{
    matches_verified_fddf, parse_command_ack, AuraPlayTogetherCommand, AURA_NOTIFY_UUID,
    AURA_VENDOR_SERVICE_UUID, AURA_WRITE_UUID, HARMAN_FDDF_UUID,
};
use crate::aura_wake::{
    acquire_aura_wake, AuraWakeBluez, AuraWakeCleanup, AuraWakeFailure, AuraWakeFddfObserver,
    AuraWakeIoFailure, AuraWakeTimings, FddfAddressKind, FreshFddfObservation, StableAddressKind,
    StableAuraState, A2DP_SINK_PROFILE_UUID, AURA_STUDIO_5_PRODUCT_ID,
};
use crate::backend::AuraAcquisitionRoute;
use crate::model::DeviceIdentity;

const DEFAULT_ADAPTER: &str = "hci0";
const DEFAULT_SCAN_ATTEMPTS: u8 = 3;
const DEFAULT_SCAN_WINDOW: Duration = Duration::from_secs(30);
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(15);
const DEFAULT_TOTAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(150);
const DEFAULT_DEVICE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_GATT_SETUP_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_COMMAND_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const STABLE_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MEDIA_CONTROL_RELEASE_POLL: Duration = Duration::from_millis(100);
const BLUEZ_SERVICE: &str = "org.bluez";
const MEDIA_CONTROL_INTERFACE: &str = "org.bluez.MediaControl1";
const MEDIA_CONTROL_CONNECTED_PROPERTY: &str = "Connected";
const MAX_SCAN_ATTEMPTS: u8 = 10;
const MAX_TOTAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_STALE_NOTIFICATIONS: usize = 32;
const ATT_PSM: u16 = 0x001f;
const ATT_FIXED_CID: u16 = 0x0004;
const ATT_WRITE_REQUEST: u8 = 0x12;
const ATT_WRITE_RESPONSE: u8 = 0x13;
const ATT_WRITE_COMMAND: u8 = 0x52;
const ATT_HANDLE_VALUE_NOTIFICATION: u8 = 0x1b;
const ATT_AURA_WRITE_HANDLE: u16 = 0x03ea;
const ATT_AURA_NOTIFY_HANDLE: u16 = 0x03ec;
const ATT_AURA_CCCD_HANDLE: u16 = 0x03ed;
const ATT_CCCD_NOTIFY_ENABLED: [u8; 2] = [0x01, 0x00];
const ATT_RECEIVE_BUFFER_SIZE: usize = 517;

/// Bounded discovery and command timings.
///
/// The defaults permit at most three 30-second active scans with at most two
/// 15-second gaps.  The 150-second outer deadline is always the hard bound for
/// discovery, connection and GATT setup together.  Time consumed by a failed
/// connection/setup can therefore truncate a later retry; three complete
/// scan+connect+setup rounds are intentionally not promised.
///
/// Validation reserves every nominal scan/delay window plus one connection
/// and one GATT-setup budget inside that hard bound.  It does not multiply the
/// connection/setup budgets by the retry count.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AuraBluezTimings {
    scan_attempts: u8,
    scan_window: Duration,
    retry_delay: Duration,
    total_connect_timeout: Duration,
    device_connect_timeout: Duration,
    gatt_setup_timeout: Duration,
    command_write_timeout: Duration,
    ack_timeout: Duration,
}

impl AuraBluezTimings {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scan_attempts: u8,
        scan_window: Duration,
        retry_delay: Duration,
        total_connect_timeout: Duration,
        device_connect_timeout: Duration,
        gatt_setup_timeout: Duration,
        command_write_timeout: Duration,
        ack_timeout: Duration,
    ) -> Result<Self, AuraTransportError> {
        let timings = Self {
            scan_attempts,
            scan_window,
            retry_delay,
            total_connect_timeout,
            device_connect_timeout,
            gatt_setup_timeout,
            command_write_timeout,
            ack_timeout,
        };
        timings.validate()?;
        Ok(timings)
    }

    fn validate(self) -> Result<(), AuraTransportError> {
        if self.scan_attempts == 0
            || self.scan_attempts > MAX_SCAN_ATTEMPTS
            || self.scan_window.is_zero()
            || self.total_connect_timeout.is_zero()
            || self.total_connect_timeout > MAX_TOTAL_CONNECT_TIMEOUT
            || self.device_connect_timeout.is_zero()
            || self.gatt_setup_timeout.is_zero()
            || self.command_write_timeout.is_zero()
            || self.ack_timeout.is_zero()
        {
            return Err(AuraTransportError::invalid_configuration());
        }

        let reserved_budget = self
            .scan_window
            .checked_mul(u32::from(self.scan_attempts))
            .and_then(|duration| {
                self.retry_delay
                    .checked_mul(u32::from(self.scan_attempts.saturating_sub(1)))
                    .and_then(|delays| duration.checked_add(delays))
            })
            .and_then(|duration| duration.checked_add(self.device_connect_timeout))
            .and_then(|duration| duration.checked_add(self.gatt_setup_timeout))
            .ok_or_else(AuraTransportError::invalid_configuration)?;
        if reserved_budget > self.total_connect_timeout {
            return Err(AuraTransportError::invalid_configuration());
        }
        Ok(())
    }

    #[cfg(test)]
    fn planned_scan_and_delay_budget(self) -> Duration {
        self.scan_window * u32::from(self.scan_attempts)
            + self.retry_delay * u32::from(self.scan_attempts.saturating_sub(1))
    }

    #[cfg(test)]
    fn hypothetical_complete_attempts_budget(self) -> Duration {
        (self.scan_window + self.device_connect_timeout + self.gatt_setup_timeout)
            * u32::from(self.scan_attempts)
            + self.retry_delay * u32::from(self.scan_attempts.saturating_sub(1))
    }
}

impl Default for AuraBluezTimings {
    fn default() -> Self {
        Self {
            scan_attempts: DEFAULT_SCAN_ATTEMPTS,
            scan_window: DEFAULT_SCAN_WINDOW,
            retry_delay: DEFAULT_RETRY_DELAY,
            total_connect_timeout: DEFAULT_TOTAL_CONNECT_TIMEOUT,
            device_connect_timeout: DEFAULT_DEVICE_CONNECT_TIMEOUT,
            gatt_setup_timeout: DEFAULT_GATT_SETUP_TIMEOUT,
            command_write_timeout: DEFAULT_COMMAND_WRITE_TIMEOUT,
            ack_timeout: DEFAULT_ACK_TIMEOUT,
        }
    }
}

impl fmt::Debug for AuraBluezTimings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuraBluezTimings")
            .field("scan_attempts", &self.scan_attempts)
            .field("scan_window", &self.scan_window)
            .field("retry_delay", &self.retry_delay)
            .field("total_connect_timeout", &self.total_connect_timeout)
            .field("device_connect_timeout", &self.device_connect_timeout)
            .field("gatt_setup_timeout", &self.gatt_setup_timeout)
            .field("command_write_timeout", &self.command_write_timeout)
            .field("ack_timeout", &self.ack_timeout)
            .finish()
    }
}

/// Non-identifying native BlueZ configuration.
#[derive(Clone, PartialEq, Eq)]
pub struct AuraBluezConfig {
    adapter_name: String,
    timings: AuraBluezTimings,
}

impl AuraBluezConfig {}

impl Default for AuraBluezConfig {
    fn default() -> Self {
        Self {
            adapter_name: DEFAULT_ADAPTER.to_string(),
            timings: AuraBluezTimings::default(),
        }
    }
}

impl fmt::Debug for AuraBluezConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuraBluezConfig")
            .field("adapter_name", &self.adapter_name)
            .field("timings", &self.timings)
            .finish()
    }
}

#[cfg(test)]
fn valid_adapter_name(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("hci") else {
        return false;
    };
    !suffix.is_empty() && suffix.len() <= 4 && suffix.bytes().all(|byte| byte.is_ascii_digit())
}

/// Sanitized native Aura transport health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraHealth {
    Offline,
    Ready,
    Unavailable,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuraBearerTransport {
    Le,
    BrEdr,
}

/// Closed native Aura failure vocabulary.
///
/// This type intentionally stores no source error.  In particular, a
/// `bluer::Error` is mapped immediately and can never escape through Debug,
/// Display, serialization, or a caller log.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AuraFailureReason {
    InvalidConfiguration,
    RuntimeUnavailable,
    AdapterUnavailable,
    AdapterPoweredOff,
    DiscoveryUnavailable,
    VerifiedAdvertisementNotFound,
    DeviceConnectionFailed,
    WakeProfileConnectFailed,
    WakeFddfTimedOut,
    WakeFddfInvalid,
    WakeFddfUnavailable,
    WakeProfileReleaseFailed,
    GattProfileInvalid,
    NotificationSetupFailed,
    TransportNotReady,
    NotificationQueueInvalid,
    WriteFailed,
    AcknowledgementTimedOut,
    AcknowledgementChannelClosed,
    UnexpectedAcknowledgement,
    #[allow(dead_code)]
    DisconnectFailed,
}

impl AuraFailureReason {
    const fn label(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_configuration",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::AdapterUnavailable => "adapter_unavailable",
            Self::AdapterPoweredOff => "adapter_powered_off",
            Self::DiscoveryUnavailable => "discovery_unavailable",
            Self::VerifiedAdvertisementNotFound => "verified_advertisement_not_found",
            Self::DeviceConnectionFailed => "device_connection_failed",
            Self::WakeProfileConnectFailed => "wake_profile_connect_failed",
            Self::WakeFddfTimedOut => "wake_fddf_timed_out",
            Self::WakeFddfInvalid => "wake_fddf_invalid",
            Self::WakeFddfUnavailable => "wake_fddf_unavailable",
            Self::WakeProfileReleaseFailed => "wake_profile_release_failed",
            Self::GattProfileInvalid => "gatt_profile_invalid",
            Self::NotificationSetupFailed => "notification_setup_failed",
            Self::TransportNotReady => "transport_not_ready",
            Self::NotificationQueueInvalid => "notification_queue_invalid",
            Self::WriteFailed => "write_failed",
            Self::AcknowledgementTimedOut => "acknowledgement_timed_out",
            Self::AcknowledgementChannelClosed => "acknowledgement_channel_closed",
            Self::UnexpectedAcknowledgement => "unexpected_acknowledgement",
            Self::DisconnectFailed => "disconnect_failed",
        }
    }
}

impl fmt::Debug for AuraFailureReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

impl fmt::Display for AuraFailureReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

impl std::error::Error for AuraFailureReason {}

/// Sanitized construction or lifecycle error.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AuraTransportError {
    reason: AuraFailureReason,
}

impl AuraTransportError {
    const fn invalid_configuration() -> Self {
        Self {
            reason: AuraFailureReason::InvalidConfiguration,
        }
    }

    fn new(reason: AuraFailureReason) -> Self {
        Self { reason }
    }

    pub(crate) const fn reason(self) -> AuraFailureReason {
        self.reason
    }

    #[cfg(test)]
    pub(crate) const fn from_reason_for_test(reason: AuraFailureReason) -> Self {
        Self { reason }
    }
}

impl fmt::Debug for AuraTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuraTransportError")
            .field("reason", &self.reason)
            .finish()
    }
}

impl fmt::Display for AuraTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl std::error::Error for AuraTransportError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuraActionFailure {
    reason: AuraFailureReason,
}

impl AuraActionFailure {
    const fn new(reason: AuraFailureReason) -> Self {
        Self { reason }
    }

    pub(crate) const fn reason(self) -> AuraFailureReason {
        self.reason
    }

    #[cfg(test)]
    pub(crate) const fn from_reason_for_test(reason: AuraFailureReason) -> Self {
        Self::new(reason)
    }
}

/// Conservative command result.
///
/// Once the fixed AA frame write has begun, every error, timeout, disconnect,
/// malformed/wrong notification, or notification-stream closure is
/// `OutcomeUnknown`.  Callers must not retry that action automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum AuraActionResult {
    Accepted,
    RejectedBeforeSend(AuraActionFailure),
    OutcomeUnknown(AuraActionFailure),
}

trait AuraIo {
    fn connect_verified(
        &mut self,
        expected_identity: DeviceIdentity,
    ) -> Result<(), AuraFailureReason>;
    fn health(&mut self) -> AuraHealth;
    fn prepare_command(&mut self) -> Result<(), AuraFailureReason>;
    fn begin_write(&mut self, command: AuraPlayTogetherCommand) -> Result<(), AuraFailureReason>;
    fn wait_for_ack(&mut self) -> Result<AckObservation, AuraFailureReason>;
    fn shutdown(&mut self) -> Result<(), AuraFailureReason>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AckObservation {
    Accepted,
    Unexpected,
}

struct AuraControlCore<I: AuraIo> {
    io: I,
    unresolved: Option<AuraActionFailure>,
}

impl<I: AuraIo> AuraControlCore<I> {
    fn new(io: I) -> Self {
        Self {
            io,
            unresolved: None,
        }
    }

    fn connect_verified(
        &mut self,
        expected_identity: DeviceIdentity,
    ) -> Result<(), AuraTransportError> {
        if self.unresolved.is_some() {
            // An uncertain write may have changed the peer.  A health probe is
            // not enough to clear it; tear down the bearer before reconnecting.
            self.io.shutdown().map_err(AuraTransportError::new)?;
        }
        self.io
            .connect_verified(expected_identity)
            .map_err(AuraTransportError::new)?;
        self.unresolved = None;
        Ok(())
    }

    fn health(&mut self) -> AuraHealth {
        if self.unresolved.is_some() {
            AuraHealth::OutcomeUnknown
        } else {
            self.io.health()
        }
    }

    fn command(&mut self, command: AuraPlayTogetherCommand) -> AuraActionResult {
        if let Some(failure) = self.unresolved {
            return AuraActionResult::OutcomeUnknown(failure);
        }
        if self.io.health() != AuraHealth::Ready {
            return AuraActionResult::RejectedBeforeSend(AuraActionFailure::new(
                AuraFailureReason::TransportNotReady,
            ));
        }
        if let Err(reason) = self.io.prepare_command() {
            return AuraActionResult::RejectedBeforeSend(AuraActionFailure::new(reason));
        }

        // From this exact call onward the implementation cannot prove that
        // the kernel/the peer observed zero bytes.
        if let Err(reason) = self.io.begin_write(command) {
            return self.latch_unknown(reason);
        }
        match self.io.wait_for_ack() {
            Ok(AckObservation::Accepted) => AuraActionResult::Accepted,
            Ok(AckObservation::Unexpected) => {
                self.latch_unknown(AuraFailureReason::UnexpectedAcknowledgement)
            }
            Err(reason) => self.latch_unknown(reason),
        }
    }

    fn latch_unknown(&mut self, reason: AuraFailureReason) -> AuraActionResult {
        let failure = AuraActionFailure::new(reason);
        self.unresolved = Some(failure);
        AuraActionResult::OutcomeUnknown(failure)
    }

    fn shutdown(&mut self) -> Result<(), AuraTransportError> {
        let result = self.io.shutdown().map_err(AuraTransportError::new);
        if result.is_ok() {
            self.unresolved = None;
        }
        result
    }
}

/// Persistent native Aura bearer for Ubuntu bluetoothd.
///
/// Construction performs no Bluetooth I/O. `connect_verified` performs the
/// stable classic ATT attempt or bounded active discovery, then retains one
/// exact kernel ATT socket with notifications enabled for repeated actions.
pub struct BluezAuraTransport {
    core: AuraControlCore<BluezIo>,
}

impl BluezAuraTransport {
    pub fn new(config: AuraBluezConfig) -> Result<Self, AuraTransportError> {
        config.timings.validate()?;
        let io = BluezIo::new(config)?;
        Ok(Self {
            core: AuraControlCore::new(io),
        })
    }

    pub fn connect_verified(
        &mut self,
        expected_identity: DeviceIdentity,
    ) -> Result<(), AuraTransportError> {
        self.core.connect_verified(expected_identity)
    }

    pub fn health(&mut self) -> AuraHealth {
        self.core.health()
    }

    pub(crate) fn transport(&self) -> Option<AuraBearerTransport> {
        self.core.io.bearer.as_ref().map(|bearer| bearer.transport)
    }

    pub(crate) fn acquisition_route(&self) -> AuraAcquisitionRoute {
        self.core
            .io
            .bearer
            .as_ref()
            .map_or(AuraAcquisitionRoute::Unresolved, |bearer| {
                bearer.acquisition_route
            })
    }

    pub fn start(&mut self) -> AuraActionResult {
        self.core.command(AuraPlayTogetherCommand::On)
    }

    pub fn stop(&mut self) -> AuraActionResult {
        self.core.command(AuraPlayTogetherCommand::Off)
    }

    pub fn shutdown(&mut self) -> Result<(), AuraTransportError> {
        self.core.shutdown()
    }
}

impl fmt::Debug for BluezAuraTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BluezAuraTransport")
            .field("connection", &"redacted")
            .finish()
    }
}

struct BluezIo {
    runtime: Runtime,
    config: AuraBluezConfig,
    bearer: Option<RawAttBearer>,
}

impl Drop for BluezIo {
    fn drop(&mut self) {
        // Drop the ATT socket, Device and Session while the private
        // current-thread runtime still exists. Graceful service exit calls
        // `shutdown` first; this fallback also covers early returns/panics.
        drop(self.bearer.take());
    }
}

type FreshDeviceEventStream = Pin<Box<dyn Stream<Item = (Address, DeviceEvent)>>>;

struct RawAttBearer {
    _session: Session,
    _device: Device,
    expected_identity: DeviceIdentity,
    transport: AuraBearerTransport,
    acquisition_route: AuraAcquisitionRoute,
    socket: SeqPacket,
}

trait ColdConnectOps {
    type Bearer;

    fn connect_stable(
        &mut self,
        route: AuraAcquisitionRoute,
    ) -> Result<Self::Bearer, AuraFailureReason>;
    fn wake_once(&mut self) -> Result<(), AuraFailureReason>;
    fn connect_le(&mut self) -> Result<Self::Bearer, AuraFailureReason>;
}

/// Exact cold-connect order derived from the bounded phone wake capture.
///
/// The temporary A2DP wake is attempted at most once, and only after the
/// initial stable raw ATT setup failed before any role write. A successful wake
/// earns one new stable raw ATT attempt. Every still-eligible setup failure then
/// falls through to the pre-existing fresh-FDDF LE path.
fn connect_cold_bearer<O: ColdConnectOps>(ops: &mut O) -> Result<O::Bearer, AuraFailureReason> {
    match ops.connect_stable(AuraAcquisitionRoute::StableDirect) {
        Ok(bearer) => return Ok(bearer),
        Err(reason) if raw_att_retry_eligible(reason) => {}
        Err(reason) => return Err(reason),
    }

    let fallback_failure = match ops.wake_once() {
        Ok(()) => match ops.connect_stable(AuraAcquisitionRoute::A2dpWakeThenStable) {
            Ok(bearer) => return Ok(bearer),
            Err(reason) if raw_att_retry_eligible(reason) => reason,
            Err(reason) => return Err(reason),
        },
        // An unconfirmed A2DP release forbids every raw ATT fallback. Other
        // wake failures have already confirmed cleanup and may safely use the
        // original LE acquisition path.
        Err(AuraFailureReason::WakeProfileReleaseFailed) => {
            return Err(AuraFailureReason::WakeProfileReleaseFailed)
        }
        Err(reason) => reason,
    };

    match ops.connect_le() {
        Ok(bearer) => Ok(bearer),
        Err(_) if wake_stage_failure(fallback_failure) => Err(fallback_failure),
        Err(AuraFailureReason::VerifiedAdvertisementNotFound) => Err(fallback_failure),
        Err(reason) => Err(reason),
    }
}

struct NativeColdConnectOps<'a> {
    runtime: &'a Runtime,
    config: AuraBluezConfig,
    expected_identity: DeviceIdentity,
    overall_deadline: Instant,
}

impl ColdConnectOps for NativeColdConnectOps<'_> {
    type Bearer = RawAttBearer;

    fn connect_stable(
        &mut self,
        route: AuraAcquisitionRoute,
    ) -> Result<Self::Bearer, AuraFailureReason> {
        self.runtime.block_on(connect_raw_att(
            self.config.clone(),
            self.expected_identity,
            self.overall_deadline,
            route,
        ))
    }

    fn wake_once(&mut self) -> Result<(), AuraFailureReason> {
        let mut bluez = NativeAuraWakeBluez {
            runtime: self.runtime,
            config: self.config.clone(),
            overall_deadline: self.overall_deadline,
        };
        let mut observer = NativeAuraWakeFddfObserver {
            runtime: self.runtime,
            config: self.config.clone(),
            overall_deadline: self.overall_deadline,
        };
        acquire_aura_wake(
            &mut bluez,
            &mut observer,
            self.expected_identity,
            AuraWakeTimings::default(),
        )
        .map(|_| ())
        .map_err(|error| map_wake_error(error.failure, error.cleanup))
    }

    fn connect_le(&mut self) -> Result<Self::Bearer, AuraFailureReason> {
        self.runtime.block_on(connect_le_raw(
            self.config.clone(),
            self.expected_identity,
            self.overall_deadline,
        ))
    }
}

fn map_wake_error(failure: AuraWakeFailure, cleanup: AuraWakeCleanup) -> AuraFailureReason {
    if cleanup == AuraWakeCleanup::ReleaseFailed {
        return AuraFailureReason::WakeProfileReleaseFailed;
    }
    match failure {
        AuraWakeFailure::ProfileConnectFailed => AuraFailureReason::WakeProfileConnectFailed,
        AuraWakeFailure::FreshFddfTimedOut => AuraFailureReason::WakeFddfTimedOut,
        AuraWakeFailure::FreshFddfInvalid => AuraFailureReason::WakeFddfInvalid,
        AuraWakeFailure::FreshFddfUnavailable => AuraFailureReason::WakeFddfUnavailable,
        AuraWakeFailure::ProfileReleaseFailed => AuraFailureReason::WakeProfileReleaseFailed,
        AuraWakeFailure::InvalidConfiguration => AuraFailureReason::InvalidConfiguration,
        AuraWakeFailure::StableDeviceUnavailable
        | AuraWakeFailure::StableIdentityMismatch
        | AuraWakeFailure::StableAddressInvalid
        | AuraWakeFailure::StableDeviceNotPaired
        | AuraWakeFailure::StableDeviceNotTrusted
        | AuraWakeFailure::StableDeviceBlocked => AuraFailureReason::DeviceConnectionFailed,
    }
}

const fn wake_stage_failure(reason: AuraFailureReason) -> bool {
    matches!(
        reason,
        AuraFailureReason::WakeProfileConnectFailed
            | AuraFailureReason::WakeFddfTimedOut
            | AuraFailureReason::WakeFddfInvalid
            | AuraFailureReason::WakeFddfUnavailable
            | AuraFailureReason::WakeProfileReleaseFailed
    )
}

struct NativeAuraWakeBluez<'a> {
    runtime: &'a Runtime,
    config: AuraBluezConfig,
    overall_deadline: Instant,
}

struct NativeAuraWakeFddfObserver<'a> {
    runtime: &'a Runtime,
    config: AuraBluezConfig,
    overall_deadline: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaControlReleaseObservation {
    Released,
    Connected,
    Invalid,
}

fn classify_media_control_release(
    interfaces: Option<&HashMap<String, PropMap>>,
) -> MediaControlReleaseObservation {
    let Some(interfaces) = interfaces else {
        return MediaControlReleaseObservation::Released;
    };
    let Some(properties) = interfaces.get(MEDIA_CONTROL_INTERFACE) else {
        return MediaControlReleaseObservation::Released;
    };
    let Some(value) = properties.get(MEDIA_CONTROL_CONNECTED_PROPERTY) else {
        return MediaControlReleaseObservation::Invalid;
    };
    match dbus::arg::cast::<bool>(&*value.0).copied() {
        Some(false) => MediaControlReleaseObservation::Released,
        Some(true) => MediaControlReleaseObservation::Connected,
        None => MediaControlReleaseObservation::Invalid,
    }
}

fn exact_device_object_path(
    adapter_name: &str,
    expected_identity: DeviceIdentity,
) -> Result<dbus::Path<'static>, AuraWakeIoFailure> {
    let address = expected_identity.binary();
    let path = format!(
        "/org/bluez/{adapter_name}/dev_{:02X}_{:02X}_{:02X}_{:02X}_{:02X}_{:02X}",
        address[0], address[1], address[2], address[3], address[4], address[5]
    );
    dbus::Path::new(path).map_err(|_| AuraWakeIoFailure::Rejected)
}

fn poll_media_control_release<F, P>(
    deadline: Instant,
    mut observe: F,
    mut pause: P,
) -> Result<bool, AuraWakeIoFailure>
where
    F: FnMut(Duration) -> Result<MediaControlReleaseObservation, AuraWakeIoFailure>,
    P: FnMut(Duration),
{
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(AuraWakeIoFailure::TimedOut);
        }
        match observe(deadline.saturating_duration_since(now))? {
            MediaControlReleaseObservation::Released => return Ok(true),
            MediaControlReleaseObservation::Invalid => return Err(AuraWakeIoFailure::Rejected),
            MediaControlReleaseObservation::Connected => {}
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(AuraWakeIoFailure::TimedOut);
        }
        pause(std::cmp::min(
            MEDIA_CONTROL_RELEASE_POLL,
            deadline.saturating_duration_since(now),
        ));
    }
}

fn wake_operation_deadline(
    overall_deadline: Instant,
    requested: Duration,
) -> Result<Instant, AuraWakeIoFailure> {
    let now = Instant::now();
    if now >= overall_deadline || requested.is_zero() {
        return Err(AuraWakeIoFailure::TimedOut);
    }
    Ok(std::cmp::min(overall_deadline, now + requested))
}

async fn wake_timeout_at<T, E, F>(deadline: Instant, future: F) -> Result<T, AuraWakeIoFailure>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    time::timeout_at(deadline, future)
        .await
        .map_err(|_| AuraWakeIoFailure::TimedOut)?
        .map_err(|_| AuraWakeIoFailure::Unavailable)
}

async fn exact_stable_wake_device(
    config: &AuraBluezConfig,
    expected_identity: DeviceIdentity,
    deadline: Instant,
) -> Result<(Session, Device), AuraWakeIoFailure> {
    let session = wake_timeout_at(deadline, Session::new()).await?;
    let adapter = session
        .adapter(&config.adapter_name)
        .map_err(|_| AuraWakeIoFailure::Unavailable)?;
    if !wake_timeout_at(deadline, adapter.is_powered()).await? {
        return Err(AuraWakeIoFailure::Unavailable);
    }
    let expected_address = Address::new(expected_identity.binary());
    let device = adapter
        .device(expected_address)
        .map_err(|_| AuraWakeIoFailure::Unavailable)?;
    if device.address() != expected_address {
        return Err(AuraWakeIoFailure::Rejected);
    }
    Ok((session, device))
}

impl AuraWakeBluez for NativeAuraWakeBluez<'_> {
    fn inspect_stable_device(
        &mut self,
        expected_identity: DeviceIdentity,
    ) -> Result<StableAuraState, AuraWakeIoFailure> {
        let deadline = wake_operation_deadline(self.overall_deadline, STABLE_CONNECT_TIMEOUT)?;
        self.runtime.block_on(async {
            let (_session, device) =
                exact_stable_wake_device(&self.config, expected_identity, deadline).await?;
            let address_type = wake_timeout_at(deadline, device.address_type()).await?;
            let paired = wake_timeout_at(deadline, device.is_paired()).await?;
            let trusted = wake_timeout_at(deadline, device.is_trusted()).await?;
            let blocked = wake_timeout_at(deadline, device.is_blocked()).await?;
            Ok(StableAuraState {
                exact_identity: expected_identity.matches_binary(&device.address().0),
                address_kind: if matches!(address_type, AddressType::BrEdr | AddressType::LePublic)
                {
                    StableAddressKind::Public
                } else {
                    StableAddressKind::Other
                },
                paired,
                trusted,
                blocked,
            })
        })
    }

    fn connect_profile(
        &mut self,
        expected_identity: DeviceIdentity,
        profile_uuid: &'static str,
        timeout: Duration,
    ) -> Result<(), AuraWakeIoFailure> {
        if profile_uuid != A2DP_SINK_PROFILE_UUID {
            return Err(AuraWakeIoFailure::Rejected);
        }
        let uuid = profile_uuid
            .parse::<Uuid>()
            .map_err(|_| AuraWakeIoFailure::Rejected)?;
        let deadline = wake_operation_deadline(self.overall_deadline, timeout)?;
        self.runtime.block_on(async {
            let (_session, device) =
                exact_stable_wake_device(&self.config, expected_identity, deadline).await?;
            wake_timeout_at(deadline, device.connect_profile(&uuid)).await
        })
    }

    fn disconnect_profile(
        &mut self,
        expected_identity: DeviceIdentity,
        profile_uuid: &'static str,
        timeout: Duration,
    ) -> Result<(), AuraWakeIoFailure> {
        if profile_uuid != A2DP_SINK_PROFILE_UUID {
            return Err(AuraWakeIoFailure::Rejected);
        }
        let uuid = profile_uuid
            .parse::<Uuid>()
            .map_err(|_| AuraWakeIoFailure::Rejected)?;
        let deadline = wake_operation_deadline(self.overall_deadline, timeout)?;
        self.runtime.block_on(async {
            let (_session, device) =
                exact_stable_wake_device(&self.config, expected_identity, deadline).await?;
            wake_timeout_at(deadline, device.disconnect_profile(&uuid)).await
        })
    }

    fn wait_profile_released(
        &mut self,
        expected_identity: DeviceIdentity,
        profile_uuid: &'static str,
        timeout: Duration,
    ) -> Result<bool, AuraWakeIoFailure> {
        if profile_uuid != A2DP_SINK_PROFILE_UUID {
            return Err(AuraWakeIoFailure::Rejected);
        }
        let deadline = wake_operation_deadline(self.overall_deadline, timeout)?;
        let device_disconnected = self.runtime.block_on(async {
            let (_session, device) =
                exact_stable_wake_device(&self.config, expected_identity, deadline).await?;
            wake_timeout_at(deadline, device.is_connected())
                .await
                .map(|connected| !connected)
        })?;
        if device_disconnected {
            return Ok(true);
        }

        let object_path = exact_device_object_path(&self.config.adapter_name, expected_identity)?;
        let connection =
            BlockingDbusConnection::new_system().map_err(|_| AuraWakeIoFailure::Unavailable)?;
        poll_media_control_release(
            deadline,
            |remaining| {
                let proxy = connection.with_proxy(BLUEZ_SERVICE, "/", remaining);
                let objects = proxy
                    .get_managed_objects()
                    .map_err(|_| AuraWakeIoFailure::Unavailable)?;
                Ok(classify_media_control_release(objects.get(&object_path)))
            },
            std::thread::sleep,
        )
    }
}

impl AuraWakeFddfObserver for NativeAuraWakeFddfObserver<'_> {
    fn wait_for_fresh_fddf(
        &mut self,
        expected_identity: DeviceIdentity,
        timeout: Duration,
    ) -> Result<Option<FreshFddfObservation>, AuraWakeIoFailure> {
        let deadline = wake_operation_deadline(self.overall_deadline, timeout)?;
        self.runtime.block_on(async {
            let session = wake_timeout_at(deadline, Session::new()).await?;
            let adapter = session
                .adapter(&self.config.adapter_name)
                .map_err(|_| AuraWakeIoFailure::Unavailable)?;
            if !wake_timeout_at(deadline, adapter.is_powered()).await? {
                return Err(AuraWakeIoFailure::Unavailable);
            }
            wake_timeout_at(
                deadline,
                adapter.set_discovery_filter(aura_discovery_filter()),
            )
            .await?;
            let known = wake_timeout_at(deadline, adapter.device_addresses())
                .await?
                .into_iter()
                .collect::<HashSet<_>>();
            let service_uuid = HARMAN_FDDF_UUID
                .parse::<Uuid>()
                .map_err(|_| AuraWakeIoFailure::Rejected)?;
            let matched = scan_once(&adapter, known, service_uuid, expected_identity, deadline)
                .await
                .map_err(|_| AuraWakeIoFailure::Unavailable)?;
            Ok(matched.map(|device| FreshFddfObservation {
                fresh: true,
                address_kind: if resolvable_private_address(device.address()) {
                    FddfAddressKind::ResolvablePrivate
                } else {
                    FddfAddressKind::Other
                },
                product_id: AURA_STUDIO_5_PRODUCT_ID,
                embedded_identity_matches: true,
            }))
        })
    }
}

impl BluezIo {
    fn new(config: AuraBluezConfig) -> Result<Self, AuraTransportError> {
        validate_protocol_constants()?;
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| AuraTransportError::new(AuraFailureReason::RuntimeUnavailable))?;
        Ok(Self {
            runtime,
            config,
            bearer: None,
        })
    }
}

fn validate_protocol_constants() -> Result<(), AuraTransportError> {
    for value in [
        HARMAN_FDDF_UUID,
        AURA_VENDOR_SERVICE_UUID,
        AURA_NOTIFY_UUID,
        AURA_WRITE_UUID,
    ] {
        parse_uuid(value).map_err(AuraTransportError::new)?;
    }
    Ok(())
}

impl AuraIo for BluezIo {
    fn connect_verified(
        &mut self,
        expected_identity: DeviceIdentity,
    ) -> Result<(), AuraFailureReason> {
        if self.bearer.as_ref().is_some_and(|bearer| {
            same_verified_identity(bearer.expected_identity, expected_identity)
        }) && self.health() == AuraHealth::Ready
        {
            return Ok(());
        }
        if self.bearer.is_some() {
            self.shutdown()?;
        }
        let config = self.config.clone();
        // All acquisition phases share one hard deadline. The exact stable
        // ATT path remains first. An eligible setup failure permits one A2DP
        // wake acquisition, one post-wake stable retry, and finally the
        // pre-existing fresh-FDDF LE fallback. None of these paths can send a
        // role command; role writes remain behind AuraControlCore::command.
        let overall_deadline = Instant::now() + config.timings.total_connect_timeout;
        let mut ops = NativeColdConnectOps {
            runtime: &self.runtime,
            config,
            expected_identity,
            overall_deadline,
        };
        let bearer = connect_cold_bearer(&mut ops)?;
        self.bearer = Some(bearer);
        Ok(())
    }

    fn health(&mut self) -> AuraHealth {
        let Some(bearer) = self.bearer.as_ref() else {
            return AuraHealth::Offline;
        };
        let observed = raw_att_socket_is_healthy(&bearer.socket);
        project_bearer_health(&mut self.bearer, observed)
    }

    fn prepare_command(&mut self) -> Result<(), AuraFailureReason> {
        let Some(bearer) = self.bearer.as_mut() else {
            return Err(AuraFailureReason::TransportNotReady);
        };
        drain_raw_att_packets(&self.runtime, bearer)
    }

    fn begin_write(&mut self, command: AuraPlayTogetherCommand) -> Result<(), AuraFailureReason> {
        let Some(bearer) = self.bearer.as_mut() else {
            return Err(AuraFailureReason::TransportNotReady);
        };
        let timeout = self.config.timings.command_write_timeout;
        let frame = command.frame();
        block_on_timeout(&self.runtime, timeout, raw_att_send_command(bearer, &frame))
            .map_err(|_| AuraFailureReason::WriteFailed)?
    }

    fn wait_for_ack(&mut self) -> Result<AckObservation, AuraFailureReason> {
        let Some(bearer) = self.bearer.as_mut() else {
            return Err(AuraFailureReason::AcknowledgementChannelClosed);
        };
        block_on_timeout(
            &self.runtime,
            self.config.timings.ack_timeout,
            raw_att_wait_for_notification(bearer),
        )
        .map_err(|_| AuraFailureReason::AcknowledgementTimedOut)?
    }

    fn shutdown(&mut self) -> Result<(), AuraFailureReason> {
        let Some(bearer) = self.bearer.take() else {
            return Ok(());
        };
        // Closing only this fixed ATT channel avoids a broad BlueZ
        // Device.Disconnect that could tear down an unrelated source.
        drop(bearer);
        Ok(())
    }
}

fn project_bearer_health<T>(bearer: &mut Option<T>, observed: Result<bool, ()>) -> AuraHealth {
    match observed {
        Ok(true) => AuraHealth::Ready,
        Ok(false) => {
            drop(bearer.take());
            AuraHealth::Offline
        }
        Err(()) => {
            drop(bearer.take());
            AuraHealth::Unavailable
        }
    }
}

fn block_on_timeout<F>(
    runtime: &Runtime,
    timeout: Duration,
    future: F,
) -> Result<F::Output, time::error::Elapsed>
where
    F: std::future::Future,
{
    // `tokio::time::timeout` itself requires an entered runtime. Construct it
    // inside the async block polled by this private runtime, never as an
    // argument that Rust evaluates before `Runtime::block_on` enters.
    runtime.block_on(async move { time::timeout(timeout, future).await })
}

enum AttPdu<'a> {
    WriteResponse,
    Notification { handle: u16, value: &'a [u8] },
    Other,
}

fn att_write_request(handle: u16, value: &[u8]) -> Vec<u8> {
    let mut request = Vec::with_capacity(3 + value.len());
    request.push(ATT_WRITE_REQUEST);
    request.extend_from_slice(&handle.to_le_bytes());
    request.extend_from_slice(value);
    request
}

fn att_write_command(handle: u16, value: &[u8]) -> Vec<u8> {
    let mut command = Vec::with_capacity(3 + value.len());
    command.push(ATT_WRITE_COMMAND);
    command.extend_from_slice(&handle.to_le_bytes());
    command.extend_from_slice(value);
    command
}

fn parse_att_pdu(packet: &[u8]) -> AttPdu<'_> {
    match packet {
        [ATT_WRITE_RESPONSE] => AttPdu::WriteResponse,
        [ATT_HANDLE_VALUE_NOTIFICATION, handle_low, handle_high, value @ ..] => {
            AttPdu::Notification {
                handle: u16::from_le_bytes([*handle_low, *handle_high]),
                value,
            }
        }
        _ => AttPdu::Other,
    }
}

async fn send_exact_att_packet(
    socket: &SeqPacket,
    packet: &[u8],
    deadline: Instant,
    failure: AuraFailureReason,
) -> Result<(), AuraFailureReason> {
    let written = timeout_at(deadline, socket.send(packet), failure).await?;
    if written == packet.len() {
        Ok(())
    } else {
        Err(failure)
    }
}

async fn receive_att_packet(
    socket: &SeqPacket,
    deadline: Instant,
    failure: AuraFailureReason,
) -> Result<Vec<u8>, AuraFailureReason> {
    let mut buffer = vec![0_u8; ATT_RECEIVE_BUFFER_SIZE];
    let received = timeout_at(deadline, socket.recv(&mut buffer), failure).await?;
    if received == 0 {
        return Err(failure);
    }
    buffer.truncate(received);
    Ok(buffer)
}

async fn raw_att_send_command(
    bearer: &mut RawAttBearer,
    frame: &[u8],
) -> Result<(), AuraFailureReason> {
    // The captured JBL One runtime uses ATT Write Command (0x52), then relies
    // exclusively on the exact RX notification as its business acknowledgement.
    let request = att_write_command(ATT_AURA_WRITE_HANDLE, frame);
    let written = bearer
        .socket
        .send(&request)
        .await
        .map_err(|_| AuraFailureReason::WriteFailed)?;
    if written != request.len() {
        return Err(AuraFailureReason::WriteFailed);
    }
    Ok(())
}

async fn raw_att_wait_for_notification(
    bearer: &mut RawAttBearer,
) -> Result<AckObservation, AuraFailureReason> {
    let mut buffer = [0_u8; ATT_RECEIVE_BUFFER_SIZE];
    let received = bearer
        .socket
        .recv(&mut buffer)
        .await
        .map_err(|_| AuraFailureReason::AcknowledgementChannelClosed)?;
    if received == 0 {
        return Err(AuraFailureReason::AcknowledgementChannelClosed);
    }
    Ok(match parse_att_pdu(&buffer[..received]) {
        AttPdu::Notification { handle, value }
            if handle == ATT_AURA_NOTIFY_HANDLE && parse_command_ack(value).is_some() =>
        {
            AckObservation::Accepted
        }
        _ => AckObservation::Unexpected,
    })
}

fn drain_raw_att_packets(
    runtime: &Runtime,
    bearer: &mut RawAttBearer,
) -> Result<(), AuraFailureReason> {
    for _ in 0..MAX_STALE_NOTIFICATIONS {
        let mut buffer = [0_u8; ATT_RECEIVE_BUFFER_SIZE];
        let result = runtime.block_on(async { bearer.socket.recv(&mut buffer).now_or_never() });
        match result {
            None => return Ok(()),
            Some(Ok(0)) | Some(Err(_)) => {
                return Err(AuraFailureReason::AcknowledgementChannelClosed)
            }
            Some(Ok(_)) => continue,
        }
    }
    Err(AuraFailureReason::NotificationQueueInvalid)
}

async fn connect_le_raw(
    config: AuraBluezConfig,
    expected_identity: DeviceIdentity,
    overall_deadline: Instant,
) -> Result<RawAttBearer, AuraFailureReason> {
    let timings = config.timings;
    let session = timeout_at(
        overall_deadline,
        Session::new(),
        AuraFailureReason::AdapterUnavailable,
    )
    .await?;
    let adapter = session
        .adapter(&config.adapter_name)
        .map_err(|_| AuraFailureReason::AdapterUnavailable)?;
    let powered = timeout_at(
        overall_deadline,
        adapter.is_powered(),
        AuraFailureReason::AdapterUnavailable,
    )
    .await?;
    if !powered {
        return Err(AuraFailureReason::AdapterPoweredOff);
    }
    let local_address = timeout_at(
        overall_deadline,
        adapter.address(),
        AuraFailureReason::AdapterUnavailable,
    )
    .await?;

    let service_uuid = parse_uuid(HARMAN_FDDF_UUID)?;
    // Do not ask BlueZ to pre-filter FDDF.  The verified reference path uses
    // broad LE + DuplicateData + empty Pattern discovery and performs the
    // exact UUID/PID/stable-identity check locally.  This also avoids losing a
    // firmware advertisement whose ServiceData is populated after DeviceAdded.
    let filter = aura_discovery_filter();
    timeout_at(
        overall_deadline,
        adapter.set_discovery_filter(filter),
        AuraFailureReason::DiscoveryUnavailable,
    )
    .await?;

    // Each retry starts at strict fresh-FDDF discovery. The resulting random LE
    // address is used only as the destination of a kernel fixed ATT CID 4
    // socket; bluetoothd never connects the Device or resolves its GATT tree.
    // Every phase shares overall_deadline, so a later attempt may be truncated.
    let connected = retry_connect_attempts(timings, overall_deadline, || {
        connect_le_raw_attempt(
            &adapter,
            local_address,
            service_uuid,
            expected_identity,
            timings,
            overall_deadline,
        )
    })
    .await?;

    Ok(RawAttBearer {
        _session: session,
        _device: connected.device,
        expected_identity,
        transport: AuraBearerTransport::Le,
        acquisition_route: AuraAcquisitionRoute::FreshLe,
        socket: connected.socket,
    })
}

async fn connect_raw_att(
    config: AuraBluezConfig,
    expected_identity: DeviceIdentity,
    overall_deadline: Instant,
    acquisition_route: AuraAcquisitionRoute,
) -> Result<RawAttBearer, AuraFailureReason> {
    let timings = config.timings;
    let stable_deadline = std::cmp::min(
        overall_deadline,
        Instant::now() + STABLE_CONNECT_TIMEOUT.saturating_add(timings.gatt_setup_timeout),
    );
    let session = timeout_at(
        stable_deadline,
        Session::new(),
        AuraFailureReason::AdapterUnavailable,
    )
    .await?;
    let adapter = session
        .adapter(&config.adapter_name)
        .map_err(|_| AuraFailureReason::AdapterUnavailable)?;
    if !timeout_at(
        stable_deadline,
        adapter.is_powered(),
        AuraFailureReason::AdapterUnavailable,
    )
    .await?
    {
        return Err(AuraFailureReason::AdapterPoweredOff);
    }

    let local_address = timeout_at(
        stable_deadline,
        adapter.address(),
        AuraFailureReason::AdapterUnavailable,
    )
    .await?;
    let address = Address::new(expected_identity.binary());
    let device = adapter
        .device(address)
        .map_err(|_| AuraFailureReason::DeviceConnectionFailed)?;
    let connect_deadline = std::cmp::min(stable_deadline, Instant::now() + STABLE_CONNECT_TIMEOUT);
    let address_type = timeout_at(
        connect_deadline,
        device.address_type(),
        AuraFailureReason::DeviceConnectionFailed,
    )
    .await?;
    let paired = timeout_at(
        connect_deadline,
        device.is_paired(),
        AuraFailureReason::DeviceConnectionFailed,
    )
    .await?;
    let trusted = timeout_at(
        connect_deadline,
        device.is_trusted(),
        AuraFailureReason::DeviceConnectionFailed,
    )
    .await?;
    let blocked = timeout_at(
        connect_deadline,
        device.is_blocked(),
        AuraFailureReason::DeviceConnectionFailed,
    )
    .await?;
    if !verified_stable_device(address_type, paired, trusted, blocked) {
        return Err(AuraFailureReason::DeviceConnectionFailed);
    }

    let (local, target) = br_edr_att_socket_addresses(local_address, address);
    let socket = connect_att_socket(local, target, connect_deadline).await?;

    let setup_deadline =
        std::cmp::min(stable_deadline, Instant::now() + timings.gatt_setup_timeout);
    enable_raw_att_notifications(&socket, setup_deadline).await?;

    Ok(RawAttBearer {
        _session: session,
        _device: device,
        expected_identity,
        transport: AuraBearerTransport::BrEdr,
        acquisition_route,
        socket,
    })
}

fn br_edr_att_socket_addresses(
    local_address: Address,
    target_address: Address,
) -> (L2capSocketAddr, L2capSocketAddr) {
    (
        L2capSocketAddr::new(local_address, AddressType::BrEdr, 0),
        L2capSocketAddr::new(target_address, AddressType::BrEdr, ATT_PSM),
    )
}

fn le_fixed_att_socket_addresses(
    local_address: Address,
    target_address: Address,
) -> (L2capSocketAddr, L2capSocketAddr) {
    (
        L2capSocketAddr {
            addr: local_address,
            addr_type: AddressType::LePublic,
            psm: 0,
            cid: ATT_FIXED_CID,
        },
        L2capSocketAddr {
            addr: target_address,
            addr_type: AddressType::LeRandom,
            psm: 0,
            cid: ATT_FIXED_CID,
        },
    )
}

async fn connect_att_socket(
    local: L2capSocketAddr,
    target: L2capSocketAddr,
    deadline: Instant,
) -> Result<SeqPacket, AuraFailureReason> {
    let socket = L2capSocket::<SeqPacket>::new_seq_packet()
        .map_err(|_| AuraFailureReason::DeviceConnectionFailed)?;
    socket
        .bind(local)
        .map_err(|_| AuraFailureReason::DeviceConnectionFailed)?;
    socket
        .set_security(Security {
            // Match both verified gatttool paths. Stable identity and fresh
            // FDDF identity are checked before their respective socket opens.
            level: SecurityLevel::Low,
            key_size: 0,
        })
        .map_err(|_| AuraFailureReason::DeviceConnectionFailed)?;
    let socket = timeout_at(
        deadline,
        socket.connect(target),
        AuraFailureReason::DeviceConnectionFailed,
    )
    .await?;
    let peer = socket
        .peer_addr()
        .map_err(|_| AuraFailureReason::DeviceConnectionFailed)?;
    if peer != target {
        return Err(AuraFailureReason::DeviceConnectionFailed);
    }
    Ok(socket)
}

async fn enable_raw_att_notifications(
    socket: &SeqPacket,
    deadline: Instant,
) -> Result<(), AuraFailureReason> {
    let request = att_write_request(ATT_AURA_CCCD_HANDLE, &ATT_CCCD_NOTIFY_ENABLED);
    send_exact_att_packet(
        socket,
        &request,
        deadline,
        AuraFailureReason::NotificationSetupFailed,
    )
    .await?;
    let response =
        receive_att_packet(socket, deadline, AuraFailureReason::NotificationSetupFailed).await?;
    if matches!(parse_att_pdu(&response), AttPdu::WriteResponse) {
        Ok(())
    } else {
        Err(AuraFailureReason::GattProfileInvalid)
    }
}

fn stable_paired_address_type(address_type: AddressType) -> bool {
    matches!(address_type, AddressType::BrEdr | AddressType::LePublic)
}

fn verified_stable_device(
    address_type: AddressType,
    paired: bool,
    trusted: bool,
    blocked: bool,
) -> bool {
    stable_paired_address_type(address_type) && paired && trusted && !blocked
}

fn raw_att_retry_eligible(reason: AuraFailureReason) -> bool {
    matches!(
        reason,
        AuraFailureReason::DeviceConnectionFailed
            | AuraFailureReason::GattProfileInvalid
            | AuraFailureReason::NotificationSetupFailed
    )
}

fn raw_att_socket_is_healthy(socket: &SeqPacket) -> Result<bool, ()> {
    raw_att_fd_is_healthy(socket.as_raw_fd())
}

fn raw_att_fd_is_healthy(fd: RawFd) -> Result<bool, ()> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `descriptor` points to one initialized pollfd for the duration
    // of this non-blocking call; the SeqPacket owns the live file descriptor.
    let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if result < 0 {
        return Err(());
    }
    if !raw_att_poll_revents_are_healthy(descriptor.revents) {
        return Ok(false);
    }
    if result == 0 || descriptor.revents & libc::POLLIN == 0 {
        return Ok(true);
    }

    // A clean peer close can be reported as POLLIN without POLLHUP on a
    // SEQPACKET socket.  Treating POLLIN alone as healthy leaves a dead Aura
    // bearer cached until the next role-changing write.  A non-consuming peek
    // distinguishes queued ATT data (>0) from EOF (0); EAGAIN means the
    // readiness raced with us and the live socket remains usable.
    let mut byte = 0_u8;
    // SAFETY: `fd` is owned by the caller for this non-blocking, one-byte,
    // non-consuming probe and `byte` is a valid writable buffer.
    let received = unsafe {
        libc::recv(
            fd,
            (&mut byte as *mut u8).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if received > 0 {
        return Ok(true);
    }
    if received == 0 {
        return Ok(false);
    }

    match std::io::Error::last_os_error().raw_os_error() {
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => Ok(true),
        _ => Err(()),
    }
}

const fn raw_att_poll_revents_are_healthy(revents: i16) -> bool {
    revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0
}

fn aura_discovery_filter() -> DiscoveryFilter {
    DiscoveryFilter {
        transport: DiscoveryTransport::Le,
        duplicate_data: true,
        pattern: Some(String::new()),
        ..DiscoveryFilter::default()
    }
}

fn same_verified_identity(bound: DeviceIdentity, requested: DeviceIdentity) -> bool {
    bound == requested
}

const fn resolvable_private_address(address: Address) -> bool {
    address.0[0] >> 6 == 0b01
}

struct ConnectedRawAttempt {
    device: Device,
    socket: SeqPacket,
}

async fn connect_le_raw_attempt(
    adapter: &Adapter,
    local_address: Address,
    service_uuid: Uuid,
    expected_identity: DeviceIdentity,
    timings: AuraBluezTimings,
    overall_deadline: Instant,
) -> Result<ConnectedRawAttempt, AuraFailureReason> {
    let known = timeout_at(
        overall_deadline,
        adapter.device_addresses(),
        AuraFailureReason::DiscoveryUnavailable,
    )
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    let scan_deadline = std::cmp::min(overall_deadline, Instant::now() + timings.scan_window);
    let device = scan_once(
        adapter,
        known,
        service_uuid,
        expected_identity,
        scan_deadline,
    )
    .await?
    .ok_or(AuraFailureReason::VerifiedAdvertisementNotFound)?;

    let connect_deadline = std::cmp::min(
        overall_deadline,
        Instant::now() + timings.device_connect_timeout,
    );
    let (local, target) = le_fixed_att_socket_addresses(local_address, device.address());
    let socket = connect_att_socket(local, target, connect_deadline).await?;

    let setup_deadline = std::cmp::min(
        overall_deadline,
        Instant::now() + timings.gatt_setup_timeout,
    );
    enable_raw_att_notifications(&socket, setup_deadline).await?;
    Ok(ConnectedRawAttempt { device, socket })
}

async fn retry_connect_attempts<T, F, Fut>(
    timings: AuraBluezTimings,
    overall_deadline: Instant,
    mut attempt: F,
) -> Result<T, AuraFailureReason>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, AuraFailureReason>>,
{
    let mut last_reason = AuraFailureReason::VerifiedAdvertisementNotFound;
    for attempt_index in 0..timings.scan_attempts {
        if Instant::now() >= overall_deadline {
            return Err(last_reason);
        }
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(reason) => last_reason = reason,
        }
        if attempt_index + 1 < timings.scan_attempts && Instant::now() < overall_deadline {
            let delay_deadline =
                std::cmp::min(overall_deadline, Instant::now() + timings.retry_delay);
            time::sleep_until(delay_deadline).await;
        }
    }
    Err(last_reason)
}

async fn scan_once(
    adapter: &Adapter,
    known: HashSet<Address>,
    service_uuid: Uuid,
    expected_identity: DeviceIdentity,
    deadline: Instant,
) -> Result<Option<Device>, AuraFailureReason> {
    // Attempt to subscribe to every object present in the pre-scan snapshot
    // before acquiring the active-discovery token. For these objects, only the
    // bytes carried by a ServiceData event received after subscription are
    // evidence; that deliberately includes the short scan-start preparation
    // window so StartDiscovery cannot create an unobserved gap. A replayed
    // DeviceAdded or an unrelated property change must never unlock cached
    // ServiceData. Keep `preexisting` independent from `subscribed`: a stale
    // unrelated object disappearing during setup may be skipped, but must not
    // subsequently be misclassified as a fresh object merely because its
    // event subscription failed.
    let mut preexisting = known;
    let mut subscribed = HashSet::new();
    let mut device_events = SelectAll::<FreshDeviceEventStream>::new();
    for address in preexisting.iter().copied() {
        let Ok(device) = adapter.device(address) else {
            continue;
        };
        if let Ok(stream) = subscribe_device_events(&device, address, deadline).await {
            subscribed.insert(address);
            device_events.push(stream);
        }
    }

    // Retaining this stream retains bluer's active-discovery token.  Unlike
    // discover_devices_with_changes, it does not collapse every property
    // change into an indistinguishable DeviceAdded event.
    let adapter_events = timeout_at(
        deadline,
        adapter.discover_devices(),
        AuraFailureReason::DiscoveryUnavailable,
    )
    .await?;
    let mut adapter_events = Box::pin(adapter_events);

    loop {
        tokio::select! {
            _ = time::sleep_until(deadline) => return Ok(None),
            adapter_event = adapter_events.next() => {
                match adapter_event {
                    None => return Err(AuraFailureReason::DiscoveryUnavailable),
                    Some(AdapterEvent::DeviceRemoved(address)) => {
                        preexisting.remove(&address);
                        subscribed.remove(&address);
                    }
                    Some(AdapterEvent::PropertyChanged(_)) => {}
                    Some(AdapterEvent::DeviceAdded(address)) => {
                        // discover_devices first replays its known-object
                        // snapshot. Initial objects remain in `preexisting`
                        // even if their first subscription failed, so this
                        // branch can retry the subscription without ever
                        // inspecting their cached current properties.
                        if preexisting.contains(&address) {
                            if !subscribed.contains(&address) {
                                let Ok(device) = adapter.device(address) else {
                                    continue;
                                };
                                if let Ok(stream) = subscribe_device_events(&device, address, deadline).await {
                                    subscribed.insert(address);
                                    device_events.push(stream);
                                }
                            }
                            continue;
                        }
                        // A duplicate DeviceAdded for an object already first
                        // observed this round cannot authorize a second cached
                        // snapshot.
                        if subscribed.contains(&address) {
                            continue;
                        }

                        let Ok(device) = adapter.device(address) else {
                            continue;
                        };
                        let stream = match subscribe_device_events(&device, address, deadline).await {
                            Ok(stream) => stream,
                            Err(_) => continue,
                        };
                        subscribed.insert(address);
                        device_events.push(stream);

                        // A Device object first observed after the pre-scan
                        // snapshot is itself fresh evidence. Subscribe before
                        // reading its initial properties so a subsequent
                        // ServiceData update cannot be missed.
                        let address_type = match time::timeout_at(deadline, device.address_type()).await {
                            Ok(Ok(address_type)) => address_type,
                            Ok(Err(_)) | Err(_) => continue,
                        };
                        let service_data = match time::timeout_at(deadline, device.service_data()).await {
                            Ok(Ok(service_data)) => service_data,
                            Ok(Err(_)) | Err(_) => continue,
                        };
                        if reduce_fresh_advertisement(
                            FreshAdvertisementEvidence::NewObjectCurrentServiceData(service_data.as_ref()),
                            address_type,
                            service_uuid,
                            &expected_identity,
                        ) {
                            return Ok(Some(device));
                        }
                    }
                }
            }
            device_event = device_events.next(), if !device_events.is_empty() => {
                let Some((address, DeviceEvent::PropertyChanged(property))) = device_event else {
                    continue;
                };
                // Do not even query AddressType for unrelated changes. Most
                // importantly, never query cached ServiceData here: the map in
                // this exact event is the only accepted evidence for a known
                // object.
                if !matches!(property, DeviceProperty::ServiceData(_)) {
                    continue;
                }
                let Ok(device) = adapter.device(address) else {
                    continue;
                };
                let address_type = match time::timeout_at(deadline, device.address_type()).await {
                    Ok(Ok(address_type)) => address_type,
                    Ok(Err(_)) | Err(_) => continue,
                };
                if reduce_fresh_advertisement(
                    FreshAdvertisementEvidence::KnownDeviceProperty(&property),
                    address_type,
                    service_uuid,
                    &expected_identity,
                ) {
                    return Ok(Some(device));
                }
            }
        }
    }
}

async fn subscribe_device_events(
    device: &Device,
    address: Address,
    deadline: Instant,
) -> Result<FreshDeviceEventStream, AuraFailureReason> {
    let events = timeout_at(
        deadline,
        device.events(),
        AuraFailureReason::DiscoveryUnavailable,
    )
    .await?;
    Ok(Box::pin(events.map(move |event| (address, event))))
}

/// Evidence attributable to this scan attempt's subscribed preparation and
/// active-discovery window.
///
/// A known object contributes only a `ServiceData` property change carrying
/// the candidate map. A newly created object may contribute its first current
/// ServiceData snapshot because the object itself appeared after the pre-scan
/// snapshot. This pure reducer performs no BlueZ/property lookup: its caller
/// must supply either the exact event map or that permitted first snapshot.
enum FreshAdvertisementEvidence<'a> {
    KnownDeviceProperty(&'a DeviceProperty),
    NewObjectCurrentServiceData(Option<&'a std::collections::HashMap<Uuid, Vec<u8>>>),
}

fn reduce_fresh_advertisement(
    evidence: FreshAdvertisementEvidence<'_>,
    address_type: AddressType,
    service_uuid: Uuid,
    expected_identity: &DeviceIdentity,
) -> bool {
    let service_data = match evidence {
        FreshAdvertisementEvidence::KnownDeviceProperty(DeviceProperty::ServiceData(map)) => map,
        FreshAdvertisementEvidence::KnownDeviceProperty(_) => return false,
        FreshAdvertisementEvidence::NewObjectCurrentServiceData(Some(map)) => map,
        FreshAdvertisementEvidence::NewObjectCurrentServiceData(None) => return false,
    };
    service_data.get(&service_uuid).is_some_and(|payload| {
        verified_scan_advertisement(address_type, payload, expected_identity)
    })
}

fn verified_scan_advertisement(
    address_type: AddressType,
    fddf_payload: &[u8],
    expected_identity: &DeviceIdentity,
) -> bool {
    address_type == AddressType::LeRandom
        && matches_verified_fddf(HARMAN_FDDF_UUID, fddf_payload, expected_identity)
}

fn parse_uuid(value: &'static str) -> Result<Uuid, AuraFailureReason> {
    value
        .parse()
        .map_err(|_| AuraFailureReason::InvalidConfiguration)
}

async fn timeout_at<T, E, F>(
    deadline: Instant,
    future: F,
    timeout_reason: AuraFailureReason,
) -> Result<T, AuraFailureReason>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    time::timeout_at(deadline, future)
        .await
        .map_err(|_| timeout_reason)?
        .map_err(|_| timeout_reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};

    fn identity() -> DeviceIdentity {
        DeviceIdentity::parse("02:00:00:00:00:02").expect("fixture identity should be valid")
    }

    fn matching_payload() -> Vec<u8> {
        let mut payload = vec![0_u8; 18];
        payload[..2].copy_from_slice(&[0x2d, 0x21]);
        payload[11..17].copy_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
        payload
    }

    fn fddf_service_data(payload: Vec<u8>) -> HashMap<Uuid, Vec<u8>> {
        HashMap::from([(
            parse_uuid(HARMAN_FDDF_UUID).expect("the fixed FDDF UUID must parse"),
            payload,
        )])
    }

    #[test]
    fn default_retry_schedule_is_three_scans_and_two_delays() {
        let timings = AuraBluezTimings::default();
        assert_eq!(timings.scan_attempts, 3);
        assert_eq!(timings.scan_window, Duration::from_secs(30));
        assert_eq!(timings.retry_delay, Duration::from_secs(15));
        assert_eq!(
            timings.planned_scan_and_delay_budget(),
            Duration::from_secs(120)
        );
        assert_eq!(timings.total_connect_timeout, Duration::from_secs(150));
        assert!(timings.validate().is_ok());
    }

    #[test]
    fn synchronous_timeout_helper_enters_runtime_before_creating_timer() {
        let runtime = Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        assert_eq!(
            block_on_timeout(&runtime, Duration::from_secs(1), async { 7_u8 }),
            Ok(7)
        );
    }

    #[test]
    fn hard_total_deadline_preempts_three_hypothetically_complete_default_rounds() {
        let timings = AuraBluezTimings::default();
        assert_eq!(
            timings.hypothetical_complete_attempts_budget(),
            Duration::from_secs(210)
        );
        assert_eq!(timings.total_connect_timeout, Duration::from_secs(150));
        assert!(
            timings.total_connect_timeout < timings.hypothetical_complete_attempts_budget(),
            "three complete scan+connect+setup rounds are not promised"
        );
        assert!(timings.validate().is_ok());
    }

    #[test]
    fn custom_schedule_reserves_one_connect_and_setup_inside_hard_deadline() {
        assert!(AuraBluezTimings::new(
            2,
            Duration::from_secs(4),
            Duration::from_secs(3),
            Duration::from_secs(17),
            Duration::from_secs(3),
            Duration::from_secs(3),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .is_ok());
        assert_eq!(
            AuraBluezTimings::new(
                2,
                Duration::from_secs(4),
                Duration::from_secs(3),
                Duration::from_secs(16),
                Duration::from_secs(3),
                Duration::from_secs(3),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .unwrap_err()
            .reason(),
            AuraFailureReason::InvalidConfiguration
        );
    }

    #[test]
    fn known_rssi_event_never_unlocks_matching_cached_fddf() {
        let expected = identity();
        let cached_service_data = fddf_service_data(matching_payload());
        assert!(reduce_fresh_advertisement(
            FreshAdvertisementEvidence::NewObjectCurrentServiceData(Some(&cached_service_data)),
            AddressType::LeRandom,
            parse_uuid(HARMAN_FDDF_UUID).expect("the fixed FDDF UUID must parse"),
            &expected,
        ));

        let unrelated_change = DeviceProperty::Rssi(-40);
        assert!(!reduce_fresh_advertisement(
            FreshAdvertisementEvidence::KnownDeviceProperty(&unrelated_change),
            AddressType::LeRandom,
            parse_uuid(HARMAN_FDDF_UUID).expect("the fixed FDDF UUID must parse"),
            &expected,
        ));
    }

    #[test]
    fn known_fresh_service_data_event_is_accepted() {
        let expected = identity();
        let change = DeviceProperty::ServiceData(fddf_service_data(matching_payload()));
        assert!(reduce_fresh_advertisement(
            FreshAdvertisementEvidence::KnownDeviceProperty(&change),
            AddressType::LeRandom,
            parse_uuid(HARMAN_FDDF_UUID).expect("the fixed FDDF UUID must parse"),
            &expected,
        ));
    }

    #[test]
    fn newly_created_object_current_service_data_is_accepted() {
        let expected = identity();
        let current_service_data = fddf_service_data(matching_payload());
        assert!(reduce_fresh_advertisement(
            FreshAdvertisementEvidence::NewObjectCurrentServiceData(Some(&current_service_data)),
            AddressType::LeRandom,
            parse_uuid(HARMAN_FDDF_UUID).expect("the fixed FDDF UUID must parse"),
            &expected,
        ));
    }

    #[test]
    fn fresh_service_data_rejects_wrong_pid_or_identity() {
        let expected = identity();
        let fddf_uuid = parse_uuid(HARMAN_FDDF_UUID).expect("the fixed FDDF UUID must parse");

        let mut wrong_pid = matching_payload();
        wrong_pid[0] ^= 1;
        let wrong_pid_change = DeviceProperty::ServiceData(fddf_service_data(wrong_pid));
        assert!(!reduce_fresh_advertisement(
            FreshAdvertisementEvidence::KnownDeviceProperty(&wrong_pid_change),
            AddressType::LeRandom,
            fddf_uuid,
            &expected,
        ));

        let other_identity = DeviceIdentity::parse("02:00:00:00:00:03")
            .expect("alternate fixture identity should be valid");
        let matching_data = fddf_service_data(matching_payload());
        assert!(!reduce_fresh_advertisement(
            FreshAdvertisementEvidence::NewObjectCurrentServiceData(Some(&matching_data)),
            AddressType::LeRandom,
            fddf_uuid,
            &other_identity,
        ));
    }

    #[test]
    fn scan_candidate_requires_le_random_and_exact_fddf_identity() {
        let expected = identity();
        let mut payload = matching_payload();
        assert!(verified_scan_advertisement(
            AddressType::LeRandom,
            &payload,
            &expected
        ));
        assert!(!verified_scan_advertisement(
            AddressType::LePublic,
            &payload,
            &expected
        ));
        assert!(!verified_scan_advertisement(
            AddressType::BrEdr,
            &payload,
            &expected
        ));
        payload[16] ^= 1;
        assert!(!verified_scan_advertisement(
            AddressType::LeRandom,
            &payload,
            &expected
        ));
    }

    #[test]
    fn discovery_filter_is_broad_le_duplicate_data_with_no_uuid_prefilter() {
        let filter = aura_discovery_filter();
        assert_eq!(filter.transport, DiscoveryTransport::Le);
        assert!(filter.duplicate_data);
        assert_eq!(filter.pattern.as_deref(), Some(""));
        assert!(filter.uuids.is_empty());
    }

    #[test]
    fn cached_bearer_identity_must_equal_the_new_private_identity() {
        let bound = identity();
        let same = DeviceIdentity::parse("020000000002").expect("same fixture should parse");
        let different =
            DeviceIdentity::parse("02:00:00:00:00:03").expect("other fixture should parse");
        assert!(same_verified_identity(bound, same));
        assert!(!same_verified_identity(bound, different));
    }

    #[test]
    fn wake_observer_accepts_only_resolvable_private_random_addresses() {
        assert!(resolvable_private_address(Address::new([
            0x40, 0, 0, 0, 0, 1
        ])));
        assert!(resolvable_private_address(Address::new([
            0x7f, 0, 0, 0, 0, 1
        ])));
        assert!(!resolvable_private_address(Address::new([
            0x3f, 0, 0, 0, 0, 1
        ])));
        assert!(!resolvable_private_address(Address::new([
            0xc0, 0, 0, 0, 0, 1
        ])));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn eligible_failure_starts_another_attempt_when_deadline_remains() {
        let timings = AuraBluezTimings::new(
            3,
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_millis(50),
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .expect("short synthetic schedule should be valid");

        for first_failure in [
            AuraFailureReason::DeviceConnectionFailed,
            AuraFailureReason::GattProfileInvalid,
            AuraFailureReason::NotificationSetupFailed,
        ] {
            let mut script = VecDeque::from([Err(first_failure), Ok(7_u8)]);
            let mut calls = 0_usize;
            let result =
                retry_connect_attempts(timings, Instant::now() + Duration::from_millis(50), || {
                    calls += 1;
                    let result = script
                        .pop_front()
                        .unwrap_or(Err(AuraFailureReason::DiscoveryUnavailable));
                    async move { result }
                })
                .await;
            assert_eq!(result, Ok(7));
            assert_eq!(calls, 2, "the full cold path must be retried");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retry_exhaustion_returns_the_last_sanitized_failure() {
        let timings = AuraBluezTimings::new(
            3,
            Duration::from_millis(1),
            Duration::ZERO,
            Duration::from_millis(20),
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .expect("short synthetic schedule should be valid");
        let mut script = VecDeque::from([
            AuraFailureReason::VerifiedAdvertisementNotFound,
            AuraFailureReason::DeviceConnectionFailed,
            AuraFailureReason::NotificationSetupFailed,
        ]);
        let mut calls = 0_usize;
        let result =
            retry_connect_attempts(timings, Instant::now() + Duration::from_millis(20), || {
                calls += 1;
                let reason = script
                    .pop_front()
                    .expect("one scripted reason per configured attempt");
                async move { Err::<(), _>(reason) }
            })
            .await;
        assert_eq!(result, Err(AuraFailureReason::NotificationSetupFailed));
        assert_eq!(calls, 3);
    }

    #[test]
    fn adapter_names_are_tightly_allowlisted() {
        for valid in ["hci0", "hci1", "hci9999"] {
            assert!(valid_adapter_name(valid));
        }
        for invalid in ["", "hci", "hci10000", "../hci0", "hci0/evil", "wlan0"] {
            assert!(!valid_adapter_name(invalid));
        }
    }

    #[test]
    fn paired_stable_raw_att_accepts_dual_mode_public_but_never_random() {
        assert!(stable_paired_address_type(AddressType::BrEdr));
        assert!(stable_paired_address_type(AddressType::LePublic));
        assert!(!stable_paired_address_type(AddressType::LeRandom));
        assert!(verified_stable_device(
            AddressType::BrEdr,
            true,
            true,
            false
        ));
        for invalid in [
            (AddressType::LeRandom, true, true, false),
            (AddressType::BrEdr, false, true, false),
            (AddressType::BrEdr, true, false, false),
            (AddressType::BrEdr, true, true, true),
        ] {
            assert!(!verified_stable_device(
                invalid.0, invalid.1, invalid.2, invalid.3
            ));
        }
    }

    #[test]
    fn raw_att_wake_is_bounded_and_limited_to_pre_command_setup() {
        assert_eq!(STABLE_CONNECT_TIMEOUT, Duration::from_secs(20));
        for reason in [
            AuraFailureReason::DeviceConnectionFailed,
            AuraFailureReason::GattProfileInvalid,
            AuraFailureReason::NotificationSetupFailed,
        ] {
            assert!(raw_att_retry_eligible(reason));
        }
        for reason in [
            AuraFailureReason::InvalidConfiguration,
            AuraFailureReason::AdapterUnavailable,
            AuraFailureReason::AdapterPoweredOff,
            AuraFailureReason::WriteFailed,
            AuraFailureReason::AcknowledgementTimedOut,
        ] {
            assert!(!raw_att_retry_eligible(reason));
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ColdEvent {
        Stable(AuraAcquisitionRoute),
        Wake,
        Le(AuraAcquisitionRoute),
    }

    struct MockColdConnectOps {
        events: Vec<ColdEvent>,
        stable: VecDeque<Result<u8, AuraFailureReason>>,
        wake: Result<(), AuraFailureReason>,
        le: Result<u8, AuraFailureReason>,
    }

    impl ColdConnectOps for MockColdConnectOps {
        type Bearer = u8;

        fn connect_stable(
            &mut self,
            route: AuraAcquisitionRoute,
        ) -> Result<Self::Bearer, AuraFailureReason> {
            self.events.push(ColdEvent::Stable(route));
            self.stable
                .pop_front()
                .unwrap_or(Err(AuraFailureReason::DeviceConnectionFailed))
        }

        fn wake_once(&mut self) -> Result<(), AuraFailureReason> {
            self.events.push(ColdEvent::Wake);
            self.wake
        }

        fn connect_le(&mut self) -> Result<Self::Bearer, AuraFailureReason> {
            self.events
                .push(ColdEvent::Le(AuraAcquisitionRoute::FreshLe));
            self.le
        }
    }

    #[test]
    fn cold_connect_projects_stable_direct_route() {
        let mut ops = MockColdConnectOps {
            events: Vec::new(),
            stable: VecDeque::from([Ok(1)]),
            wake: Ok(()),
            le: Ok(2),
        };

        assert_eq!(connect_cold_bearer(&mut ops), Ok(1));
        assert_eq!(
            ops.events,
            [ColdEvent::Stable(AuraAcquisitionRoute::StableDirect)]
        );
    }

    #[test]
    fn cold_connect_projects_a2dp_wake_then_stable_route() {
        let mut ops = MockColdConnectOps {
            events: Vec::new(),
            stable: VecDeque::from([Err(AuraFailureReason::DeviceConnectionFailed), Ok(2)]),
            wake: Ok(()),
            le: Ok(3),
        };

        assert_eq!(connect_cold_bearer(&mut ops), Ok(2));
        assert_eq!(
            ops.events,
            [
                ColdEvent::Stable(AuraAcquisitionRoute::StableDirect),
                ColdEvent::Wake,
                ColdEvent::Stable(AuraAcquisitionRoute::A2dpWakeThenStable)
            ]
        );
    }

    #[test]
    fn cold_connect_uses_one_wake_then_retries_stable_before_le() {
        let mut ops = MockColdConnectOps {
            events: Vec::new(),
            stable: VecDeque::from([
                Err(AuraFailureReason::DeviceConnectionFailed),
                Err(AuraFailureReason::NotificationSetupFailed),
            ]),
            wake: Ok(()),
            le: Ok(7),
        };

        assert_eq!(connect_cold_bearer(&mut ops), Ok(7));
        assert_eq!(
            ops.events,
            [
                ColdEvent::Stable(AuraAcquisitionRoute::StableDirect),
                ColdEvent::Wake,
                ColdEvent::Stable(AuraAcquisitionRoute::A2dpWakeThenStable),
                ColdEvent::Le(AuraAcquisitionRoute::FreshLe)
            ]
        );
    }

    #[test]
    fn wake_failure_still_reaches_original_le_fallback_without_role_api() {
        let mut ops = MockColdConnectOps {
            events: Vec::new(),
            stable: VecDeque::from([Err(AuraFailureReason::DeviceConnectionFailed)]),
            wake: Err(AuraFailureReason::WakeFddfTimedOut),
            le: Ok(9),
        };

        assert_eq!(connect_cold_bearer(&mut ops), Ok(9));
        assert_eq!(
            ops.events,
            [
                ColdEvent::Stable(AuraAcquisitionRoute::StableDirect),
                ColdEvent::Wake,
                ColdEvent::Le(AuraAcquisitionRoute::FreshLe)
            ]
        );
        // `ColdConnectOps` deliberately has no role-write, playback, button,
        // phone, or group method, so this trace cannot mutate those states.
    }

    #[test]
    fn wake_stage_failures_are_closed_and_survive_a_failed_le_fallback() {
        for (failure, expected) in [
            (
                AuraWakeFailure::ProfileConnectFailed,
                AuraFailureReason::WakeProfileConnectFailed,
            ),
            (
                AuraWakeFailure::FreshFddfTimedOut,
                AuraFailureReason::WakeFddfTimedOut,
            ),
            (
                AuraWakeFailure::FreshFddfInvalid,
                AuraFailureReason::WakeFddfInvalid,
            ),
            (
                AuraWakeFailure::FreshFddfUnavailable,
                AuraFailureReason::WakeFddfUnavailable,
            ),
            (
                AuraWakeFailure::ProfileReleaseFailed,
                AuraFailureReason::WakeProfileReleaseFailed,
            ),
        ] {
            assert_eq!(map_wake_error(failure, AuraWakeCleanup::Released), expected);
            let mut ops = MockColdConnectOps {
                events: Vec::new(),
                stable: VecDeque::from([Err(AuraFailureReason::DeviceConnectionFailed)]),
                wake: Err(expected),
                le: Err(AuraFailureReason::DiscoveryUnavailable),
            };
            assert_eq!(connect_cold_bearer(&mut ops), Err(expected));
        }
        assert_eq!(
            map_wake_error(
                AuraWakeFailure::FreshFddfTimedOut,
                AuraWakeCleanup::ReleaseFailed,
            ),
            AuraFailureReason::WakeProfileReleaseFailed
        );
    }

    #[test]
    fn wake_is_never_repeated_and_noneligible_failure_stops_immediately() {
        let mut post_wake_failure = MockColdConnectOps {
            events: Vec::new(),
            stable: VecDeque::from([
                Err(AuraFailureReason::DeviceConnectionFailed),
                Err(AuraFailureReason::GattProfileInvalid),
            ]),
            wake: Ok(()),
            le: Err(AuraFailureReason::VerifiedAdvertisementNotFound),
        };
        assert_eq!(
            connect_cold_bearer(&mut post_wake_failure),
            Err(AuraFailureReason::GattProfileInvalid)
        );
        assert_eq!(
            post_wake_failure.events,
            [
                ColdEvent::Stable(AuraAcquisitionRoute::StableDirect),
                ColdEvent::Wake,
                ColdEvent::Stable(AuraAcquisitionRoute::A2dpWakeThenStable),
                ColdEvent::Le(AuraAcquisitionRoute::FreshLe)
            ]
        );

        let mut noneligible = MockColdConnectOps {
            events: Vec::new(),
            stable: VecDeque::from([Err(AuraFailureReason::AdapterPoweredOff)]),
            wake: Ok(()),
            le: Ok(1),
        };
        assert_eq!(
            connect_cold_bearer(&mut noneligible),
            Err(AuraFailureReason::AdapterPoweredOff)
        );
        assert_eq!(
            noneligible.events,
            [ColdEvent::Stable(AuraAcquisitionRoute::StableDirect)]
        );

        let mut release_unknown = MockColdConnectOps {
            events: Vec::new(),
            stable: VecDeque::from([Err(AuraFailureReason::DeviceConnectionFailed)]),
            wake: Err(AuraFailureReason::WakeProfileReleaseFailed),
            le: Ok(3),
        };
        assert_eq!(
            connect_cold_bearer(&mut release_unknown),
            Err(AuraFailureReason::WakeProfileReleaseFailed)
        );
        assert_eq!(
            release_unknown.events,
            [
                ColdEvent::Stable(AuraAcquisitionRoute::StableDirect),
                ColdEvent::Wake
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wake_phase_deadlines_are_clamped_to_the_shared_outer_deadline() {
        let overall = Instant::now() + Duration::from_millis(25);
        let phase = wake_operation_deadline(overall, Duration::from_secs(20)).unwrap();
        assert!(phase <= overall);
        time::sleep_until(overall).await;
        assert_eq!(
            wake_operation_deadline(overall, Duration::from_secs(1)),
            Err(AuraWakeIoFailure::TimedOut)
        );
    }

    fn media_control_interfaces(
        value: Option<Box<dyn dbus::arg::RefArg>>,
    ) -> HashMap<String, PropMap> {
        let mut properties = PropMap::new();
        if let Some(value) = value {
            properties.insert(
                MEDIA_CONTROL_CONNECTED_PROPERTY.to_string(),
                dbus::arg::Variant(value),
            );
        }
        HashMap::from([(MEDIA_CONTROL_INTERFACE.to_string(), properties)])
    }

    #[test]
    fn media_control_release_classification_is_exact_and_closed() {
        assert_eq!(
            classify_media_control_release(None),
            MediaControlReleaseObservation::Released
        );
        assert_eq!(
            classify_media_control_release(Some(&HashMap::new())),
            MediaControlReleaseObservation::Released
        );

        let disconnected = media_control_interfaces(Some(Box::new(false)));
        assert_eq!(
            classify_media_control_release(Some(&disconnected)),
            MediaControlReleaseObservation::Released
        );
        let connected = media_control_interfaces(Some(Box::new(true)));
        assert_eq!(
            classify_media_control_release(Some(&connected)),
            MediaControlReleaseObservation::Connected
        );

        let missing_property = media_control_interfaces(None);
        assert_eq!(
            classify_media_control_release(Some(&missing_property)),
            MediaControlReleaseObservation::Invalid
        );
        let wrong_type = media_control_interfaces(Some(Box::new("false".to_string())));
        assert_eq!(
            classify_media_control_release(Some(&wrong_type)),
            MediaControlReleaseObservation::Invalid
        );
    }

    #[test]
    fn media_control_release_poll_handles_transition_bus_error_and_timeout() {
        let mut script = VecDeque::from([
            MediaControlReleaseObservation::Connected,
            MediaControlReleaseObservation::Released,
        ]);
        let mut pauses = 0_usize;
        assert_eq!(
            poll_media_control_release(
                Instant::now() + Duration::from_secs(1),
                |_| Ok(script.pop_front().expect("one scripted observation")),
                |_| pauses += 1,
            ),
            Ok(true)
        );
        assert_eq!(pauses, 1);

        assert_eq!(
            poll_media_control_release(
                Instant::now() + Duration::from_secs(1),
                |_| Err(AuraWakeIoFailure::Unavailable),
                |_| {},
            ),
            Err(AuraWakeIoFailure::Unavailable)
        );
        assert_eq!(
            poll_media_control_release(
                Instant::now(),
                |_| Ok(MediaControlReleaseObservation::Released),
                |_| {},
            ),
            Err(AuraWakeIoFailure::TimedOut)
        );
    }

    #[test]
    fn raw_att_socket_addresses_bind_exact_adapter_and_transport_parameters() {
        let local = Address::new([0x02, 0, 0, 0, 0, 1]);
        let target = Address::new([0x02, 0, 0, 0, 0, 2]);

        let (classic_source, classic_target) = br_edr_att_socket_addresses(local, target);
        assert_eq!(classic_source.addr, local);
        assert_eq!(classic_source.addr_type, AddressType::BrEdr);
        assert_eq!(classic_source.psm, 0);
        assert_eq!(classic_source.cid, 0);
        assert_eq!(classic_target.addr, target);
        assert_eq!(classic_target.addr_type, AddressType::BrEdr);
        assert_eq!(classic_target.psm, ATT_PSM);
        assert_eq!(classic_target.cid, 0);

        let (le_source, le_target) = le_fixed_att_socket_addresses(local, target);
        assert_eq!(le_source.addr, local);
        assert_eq!(le_source.addr_type, AddressType::LePublic);
        assert_eq!(le_source.psm, 0);
        assert_eq!(le_source.cid, ATT_FIXED_CID);
        assert_eq!(le_target.addr, target);
        assert_eq!(le_target.addr_type, AddressType::LeRandom);
        assert_eq!(le_target.psm, 0);
        assert_eq!(le_target.cid, ATT_FIXED_CID);
    }

    #[test]
    fn raw_att_encodes_only_fixed_write_and_cccd_requests() {
        assert_eq!(
            att_write_request(ATT_AURA_CCCD_HANDLE, &ATT_CCCD_NOTIFY_ENABLED),
            [0x12, 0xed, 0x03, 0x01, 0x00]
        );
        let command =
            att_write_command(ATT_AURA_WRITE_HANDLE, &AuraPlayTogetherCommand::On.frame());
        assert_eq!(
            command,
            [0x52, 0xea, 0x03, 0xaa, 0x13, 0x04, 0x00, 0x3c, 0x01, 0x01]
        );
        assert_ne!(command[0], ATT_WRITE_REQUEST);
    }

    #[test]
    fn raw_att_parser_requires_exact_response_and_notification_shape() {
        assert!(matches!(parse_att_pdu(&[0x13]), AttPdu::WriteResponse));
        assert!(matches!(
            parse_att_pdu(&[0x1b, 0xec, 0x03, 0xaa, 0x00, 0x02, 0x13, 0x00]),
            AttPdu::Notification {
                handle: ATT_AURA_NOTIFY_HANDLE,
                value: [0xaa, 0x00, 0x02, 0x13, 0x00]
            }
        ));
        for malformed in [
            &[][..],
            &[0x13, 0x00][..],
            &[0x1b][..],
            &[0x1b, 0xec][..],
            &[0x01, 0x12, 0xea, 0x03, 0x03][..],
        ] {
            assert!(matches!(parse_att_pdu(malformed), AttPdu::Other));
        }
    }

    #[test]
    fn raw_att_health_rejects_kernel_error_or_hangup_flags() {
        assert!(raw_att_poll_revents_are_healthy(0));
        assert!(raw_att_poll_revents_are_healthy(libc::POLLIN));
        for unhealthy in [libc::POLLERR, libc::POLLHUP, libc::POLLNVAL] {
            assert!(!raw_att_poll_revents_are_healthy(unhealthy));
        }
    }

    #[test]
    fn raw_att_health_distinguishes_queued_packet_from_clean_peer_eof() {
        use std::os::fd::{FromRawFd, OwnedFd};

        let mut descriptors = [-1, -1];
        // SAFETY: `descriptors` has room for both returned descriptors.  Each
        // successful descriptor is immediately wrapped in `OwnedFd` below.
        let created = unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
                descriptors.as_mut_ptr(),
            )
        };
        assert_eq!(created, 0);
        // SAFETY: socketpair returned two fresh, owned descriptors.
        let local = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        // SAFETY: socketpair returned two fresh, owned descriptors.
        let peer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };

        assert_eq!(raw_att_fd_is_healthy(local.as_raw_fd()), Ok(true));
        let packet = [0x1b_u8];
        // SAFETY: the peer fd is live and `packet` is a valid readable buffer.
        let sent = unsafe {
            libc::send(
                peer.as_raw_fd(),
                packet.as_ptr().cast(),
                packet.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        assert_eq!(sent, 1);
        assert_eq!(raw_att_fd_is_healthy(local.as_raw_fd()), Ok(true));

        drop(peer);
        let mut drained = [0_u8; 1];
        // SAFETY: the local fd is live and `drained` is writable.
        let received = unsafe {
            libc::recv(
                local.as_raw_fd(),
                drained.as_mut_ptr().cast(),
                drained.len(),
                0,
            )
        };
        assert_eq!(received, 1);
        assert_eq!(raw_att_fd_is_healthy(local.as_raw_fd()), Ok(false));
    }

    #[test]
    fn eof_or_probe_failure_drops_the_stale_bearer_projection() {
        let mut bearer = Some(7_u8);
        assert_eq!(
            project_bearer_health(&mut bearer, Ok(true)),
            AuraHealth::Ready
        );
        assert_eq!(bearer, Some(7));

        assert_eq!(
            project_bearer_health(&mut bearer, Ok(false)),
            AuraHealth::Offline
        );
        assert_eq!(bearer, None, "clean EOF must force a fresh reconnect");

        let mut bearer = Some(9_u8);
        assert_eq!(
            project_bearer_health(&mut bearer, Err(())),
            AuraHealth::Unavailable
        );
        assert_eq!(bearer, None, "an unprobeable bearer must not be cached");
    }

    struct MockIo {
        health: AuraHealth,
        prepare: Result<(), AuraFailureReason>,
        writes: Vec<AuraPlayTogetherCommand>,
        write_results: VecDeque<Result<(), AuraFailureReason>>,
        ack_results: VecDeque<Result<AckObservation, AuraFailureReason>>,
        shutdown_result: Result<(), AuraFailureReason>,
        shutdowns: usize,
    }

    impl MockIo {
        fn ready() -> Self {
            Self {
                health: AuraHealth::Ready,
                prepare: Ok(()),
                writes: Vec::new(),
                write_results: VecDeque::from([Ok(())]),
                ack_results: VecDeque::from([Ok(AckObservation::Accepted)]),
                shutdown_result: Ok(()),
                shutdowns: 0,
            }
        }
    }

    impl AuraIo for MockIo {
        fn connect_verified(
            &mut self,
            _expected_identity: DeviceIdentity,
        ) -> Result<(), AuraFailureReason> {
            self.health = AuraHealth::Ready;
            Ok(())
        }

        fn health(&mut self) -> AuraHealth {
            self.health
        }

        fn prepare_command(&mut self) -> Result<(), AuraFailureReason> {
            self.prepare
        }

        fn begin_write(
            &mut self,
            command: AuraPlayTogetherCommand,
        ) -> Result<(), AuraFailureReason> {
            self.writes.push(command);
            self.write_results
                .pop_front()
                .unwrap_or(Err(AuraFailureReason::WriteFailed))
        }

        fn wait_for_ack(&mut self) -> Result<AckObservation, AuraFailureReason> {
            self.ack_results
                .pop_front()
                .unwrap_or(Err(AuraFailureReason::AcknowledgementTimedOut))
        }

        fn shutdown(&mut self) -> Result<(), AuraFailureReason> {
            self.shutdowns += 1;
            if self.shutdown_result.is_ok() {
                self.health = AuraHealth::Offline;
            }
            self.shutdown_result
        }
    }

    #[test]
    fn exact_ack_accepts_the_closed_command() {
        let mut core = AuraControlCore::new(MockIo::ready());
        assert_eq!(
            core.command(AuraPlayTogetherCommand::On),
            AuraActionResult::Accepted
        );
        assert_eq!(core.io.writes, vec![AuraPlayTogetherCommand::On]);
    }

    #[test]
    fn failure_before_write_is_rejected_before_send() {
        let mut io = MockIo::ready();
        io.prepare = Err(AuraFailureReason::NotificationQueueInvalid);
        let mut core = AuraControlCore::new(io);
        assert_eq!(
            core.command(AuraPlayTogetherCommand::Off),
            AuraActionResult::RejectedBeforeSend(AuraActionFailure::new(
                AuraFailureReason::NotificationQueueInvalid
            ))
        );
        assert!(core.io.writes.is_empty());
    }

    #[test]
    fn write_error_timeout_disconnect_and_wrong_ack_are_all_unknown() {
        for (write_result, ack_result, expected_reason) in [
            (
                Err(AuraFailureReason::WriteFailed),
                Ok(AckObservation::Accepted),
                AuraFailureReason::WriteFailed,
            ),
            (
                Ok(()),
                Err(AuraFailureReason::AcknowledgementTimedOut),
                AuraFailureReason::AcknowledgementTimedOut,
            ),
            (
                Ok(()),
                Err(AuraFailureReason::AcknowledgementChannelClosed),
                AuraFailureReason::AcknowledgementChannelClosed,
            ),
            (
                Ok(()),
                Ok(AckObservation::Unexpected),
                AuraFailureReason::UnexpectedAcknowledgement,
            ),
        ] {
            let mut io = MockIo::ready();
            io.write_results = VecDeque::from([write_result]);
            io.ack_results = VecDeque::from([ack_result]);
            let mut core = AuraControlCore::new(io);
            assert_eq!(
                core.command(AuraPlayTogetherCommand::On),
                AuraActionResult::OutcomeUnknown(AuraActionFailure::new(expected_reason))
            );
            assert_eq!(core.health(), AuraHealth::OutcomeUnknown);
        }
    }

    #[test]
    fn unknown_outcome_latches_and_prevents_a_second_write() {
        let mut io = MockIo::ready();
        io.ack_results = VecDeque::from([Err(AuraFailureReason::AcknowledgementTimedOut)]);
        let mut core = AuraControlCore::new(io);
        let first = core.command(AuraPlayTogetherCommand::On);
        let second = core.command(AuraPlayTogetherCommand::Off);
        assert_eq!(second, first);
        assert_eq!(core.io.writes.len(), 1);
    }

    #[test]
    fn reconnect_after_unknown_forces_shutdown_before_clearing_the_latch() {
        let mut io = MockIo::ready();
        io.ack_results = VecDeque::from([Err(AuraFailureReason::AcknowledgementTimedOut)]);
        let mut core = AuraControlCore::new(io);
        assert!(matches!(
            core.command(AuraPlayTogetherCommand::On),
            AuraActionResult::OutcomeUnknown(_)
        ));
        core.connect_verified(identity())
            .expect("mock reconnect should succeed");
        assert_eq!(core.health(), AuraHealth::Ready);
        assert_eq!(core.io.shutdowns, 1);
    }

    #[test]
    fn failed_recovery_shutdown_keeps_the_unknown_latch() {
        let mut io = MockIo::ready();
        io.ack_results = VecDeque::from([Err(AuraFailureReason::AcknowledgementTimedOut)]);
        io.shutdown_result = Err(AuraFailureReason::DisconnectFailed);
        let mut core = AuraControlCore::new(io);
        let uncertain = core.command(AuraPlayTogetherCommand::On);
        assert!(matches!(uncertain, AuraActionResult::OutcomeUnknown(_)));
        assert_eq!(
            core.connect_verified(identity()).unwrap_err().reason(),
            AuraFailureReason::DisconnectFailed
        );
        assert_eq!(core.health(), AuraHealth::OutcomeUnknown);
        assert_eq!(core.io.shutdowns, 1);
    }

    #[test]
    fn sanitized_errors_and_debug_never_include_transport_material() {
        let error = AuraTransportError::new(AuraFailureReason::DiscoveryUnavailable);
        let rendered = format!("{error:?} {error}");
        assert!(rendered.contains("discovery_unavailable"));
        for forbidden in ["org.bluez", "/org/bluez", "AA1304", "02:00:00"] {
            assert!(!rendered.contains(forbidden));
        }
        let transport = BluezAuraTransport::new(AuraBluezConfig::default())
            .expect("constructing the offline transport should succeed");
        assert_eq!(
            format!("{transport:?}"),
            "BluezAuraTransport { connection: \"redacted\" }"
        );
    }
}
